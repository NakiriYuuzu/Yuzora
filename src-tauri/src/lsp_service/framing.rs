// JSON-RPC Content-Length framing and executable path resolution.
//
// Pure, tauri-free helpers split out of the LSP manager: message framing
// (`frame` / `parse_frames`) and command resolution (`which`). Heavily unit
// tested here; the manager consumes them via `super`.

use std::path::{Path, PathBuf};

// Upper bound on a single *declared* JSON-RPC frame body. Real LSP messages are far
// smaller (a large completion/diagnostics payload is a few MB at most); 64 MB leaves
// ample headroom while capping a hostile or corrupt `Content-Length` so a declared
// body can never overflow arithmetic or index out of bounds. Un-framed input (no
// header terminator) is bounded separately by MAX_HEADER_BYTES.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
// Upper bound on the un-framed prefix scanned for a `\r\n\r\n` header terminator. LSP
// headers are tiny (well under 1 KB). If this many bytes accumulate without a
// terminator the stream is not LSP framing (a wrong binary's usage text, a server
// spewing garbage) and parse_frames resyncs by dropping the backlog. This both caps
// buffer growth on an un-framed stream and keeps the header scan cost bounded per
// call (no O(n²) rescans of an ever-growing buffer).
const MAX_HEADER_BYTES: usize = 8 * 1024;

// ---- pure core (heavily unit-tested, no tauri State) ------------------------

/// Wrap a message body in a Content-Length framed JSON-RPC packet.
pub fn frame(body: &str) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Extract every complete message body from an accumulation buffer, draining the
/// consumed bytes and preserving any trailing partial frame for the next read.
///
/// Two bounds keep this safe against hostile / non-LSP input (F-R4-1, F-R5-1):
///  - a *declared* body over MAX_FRAME_BYTES is rejected as malformed;
///  - an un-framed prefix (no `\r\n\r\n`) over MAX_HEADER_BYTES triggers a resync
///    that drops the backlog, keeping only the last `DELIM.len()-1` bytes so a
///    terminator can't be split across a chunk boundary.
///
/// So the buffer never exceeds ~MAX_FRAME_BYTES + MAX_HEADER_BYTES, and because a
/// valid header terminator sits near the front (or the un-framed prefix is capped),
/// the per-call header scan is bounded — no O(n²) rescans of a growing buffer.
pub fn parse_frames(buf: &mut Vec<u8>) -> Vec<String> {
    const DELIM: &[u8] = b"\r\n\r\n";
    let mut out = Vec::new();
    loop {
        let header_end = match find_subslice(buf, DELIM) {
            Some(pos) => pos,
            None => {
                // No terminator yet. If the un-framed prefix already exceeds the max
                // header size the stream isn't LSP framing — resync, keeping only the
                // last DELIM.len()-1 bytes in case a terminator straddles the boundary.
                if buf.len() > MAX_HEADER_BYTES {
                    let keep = buf.len() - (DELIM.len() - 1);
                    buf.drain(..keep);
                }
                break;
            }
        };
        if header_end > MAX_HEADER_BYTES {
            // Terminator found, but the header before it is absurdly long — treat as
            // malformed and drop through the terminator.
            buf.drain(..header_end + DELIM.len());
            continue;
        }
        let len = match content_length(&buf[..header_end]) {
            // A length above MAX_FRAME_BYTES (or an overflowing/garbage value that
            // still parsed, e.g. u64::MAX) is treated as a malformed header: drop it
            // via the same path so the stream neither spins, overflows, nor indexes
            // out of bounds waiting for a body that would exceed the cap.
            Some(n) if n <= MAX_FRAME_BYTES => n,
            _ => {
                buf.drain(..header_end + DELIM.len());
                continue;
            }
        };
        let body_start = header_end + DELIM.len();
        // Defence in depth against arithmetic overflow (len is already bounded).
        let Some(frame_end) = body_start.checked_add(len) else {
            buf.drain(..body_start);
            continue;
        };
        if buf.len() < frame_end {
            break; // body not fully arrived yet
        }
        let body = String::from_utf8_lossy(&buf[body_start..frame_end]).into_owned();
        out.push(body);
        buf.drain(..frame_end);
    }
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn content_length(header: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(header).ok()?;
    for line in text.split("\r\n") {
        if let Some(v) = line.strip_prefix("Content-Length:") {
            return v.trim().parse().ok();
        }
    }
    None
}

/// Resolve a command to an absolute path. Spike trap #1: never spawn a relative
/// name. Search the ~/.yuzora/servers/ bin dirs (T14 one-click install landing
/// spots) first, then PATH.
pub fn which(command: &str) -> Option<String> {
    if let Some(p) = resolve_in_dirs(command, &server_bin_dirs()) {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    let path_dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    resolve_in_dirs(command, &path_dirs)
}

/// First `dir/<candidate>` that is an executable file, as an absolute string.
fn resolve_in_dirs(command: &str, dirs: &[PathBuf]) -> Option<String> {
    let names = candidate_names(command);
    for dir in dirs {
        for name in &names {
            let cand = dir.join(name);
            if is_executable(&cand) {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn candidate_names(command: &str) -> Vec<String> {
    candidate_names_for(command, cfg!(windows))
}

/// Executable file-name candidates for `command`. Unix uses the bare name;
/// Windows tries `.exe` then `.cmd` (F-C: npm shims land as `.cmd`), so
/// `which` resolves the same landing spots the `server_bin_dirs_from`
/// `cfg!(windows)` branch prepares. The bare name goes last on Windows: Node's
/// install dir ships an extensionless `npm` (a Unix sh script) next to
/// `npm.cmd`, and `is_executable` on Windows is only `is_file()`, so a
/// bare-name-first order resolves a shim CreateProcessW cannot run (#11).
fn candidate_names_for(command: &str, windows: bool) -> Vec<String> {
    if windows {
        vec![
            format!("{command}.exe"),
            format!("{command}.cmd"),
            command.to_string(),
        ]
    } else {
        vec![command.to_string()]
    }
}

fn server_bin_dirs() -> Vec<PathBuf> {
    let base = dirs::home_dir()
        .unwrap_or_default()
        .join(".yuzora")
        .join("servers");
    server_bin_dirs_from(&base)
}

/// T14 (A9/A9') install landing spots under `~/.yuzora/servers`, in resolution
/// order: binary-series download root, npm private-prefix bin, pylsp venv bin
/// (`pyenv/bin` on unix, `pyenv/Scripts` on Windows).
fn server_bin_dirs_from(base: &Path) -> Vec<PathBuf> {
    let pyenv_bin = if cfg!(windows) {
        base.join("pyenv").join("Scripts")
    } else {
        base.join("pyenv").join("bin")
    };
    vec![
        base.to_path_buf(),
        base.join("npm").join("node_modules").join(".bin"),
        pyenv_bin,
    ]
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- framing pure functions ---

    #[test]
    fn frame_produces_content_length_header() {
        let out = frame("hi");
        assert_eq!(out, b"Content-Length: 2\r\n\r\nhi");
    }

    #[test]
    fn frame_parse_round_trip() {
        let mut buf = frame("{\"jsonrpc\":\"2.0\"}");
        let got = parse_frames(&mut buf);
        assert_eq!(got, vec!["{\"jsonrpc\":\"2.0\"}".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn parse_frames_handles_glued_messages() {
        let mut buf = frame("aa");
        buf.extend_from_slice(&frame("bbb"));
        let got = parse_frames(&mut buf);
        assert_eq!(got, vec!["aa".to_string(), "bbb".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn parse_frames_across_chunks_keeps_residual() {
        let full = frame("hello");
        let (head, tail) = full.split_at(10);
        let mut buf = head.to_vec();
        // Not enough bytes yet → nothing extracted, buffer preserved.
        assert!(parse_frames(&mut buf).is_empty());
        assert_eq!(buf, head.to_vec());
        buf.extend_from_slice(tail);
        assert_eq!(parse_frames(&mut buf), vec!["hello".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn parse_frames_keeps_partial_second_message() {
        let mut buf = frame("first");
        let second = frame("second");
        buf.extend_from_slice(&second[..8]); // only part of the 2nd frame
        let got = parse_frames(&mut buf);
        assert_eq!(got, vec!["first".to_string()]);
        assert_eq!(buf, second[..8].to_vec());
    }

    #[test]
    fn parse_frames_uses_content_length_not_delimiter() {
        // Body itself contains the header terminator; length-based extraction
        // must return the whole body, not stop at the embedded \r\n\r\n.
        let body = "a\r\n\r\nb";
        let mut buf = frame(body);
        assert_eq!(parse_frames(&mut buf), vec![body.to_string()]);
    }

    #[test]
    fn parse_frames_empty_buffer_returns_nothing() {
        let mut buf: Vec<u8> = Vec::new();
        assert!(parse_frames(&mut buf).is_empty());
    }

    #[test]
    fn parse_frames_rejects_overflow_content_length() {
        // F-R4-1: u64::MAX must not panic (debug add-overflow / release OOB slice)
        // and must not stall the stream — the bogus header is dropped and a
        // following well-formed frame still parses.
        let mut buf = b"Content-Length: 18446744073709551615\r\n\r\nX".to_vec();
        assert!(parse_frames(&mut buf).is_empty());
        // Reader resyncs; a subsequent well-formed frame parses normally.
        buf.clear();
        buf.extend_from_slice(&frame("ok"));
        assert_eq!(parse_frames(&mut buf), vec!["ok".to_string()]);
    }

    #[test]
    fn parse_frames_rejects_oversize_content_length() {
        // F-R4-1: a "legal but absurd" length above MAX_FRAME_BYTES is malformed —
        // the buffer must not grow unbounded waiting for a 64MB+ body, and a later
        // normal frame still parses.
        let len = MAX_FRAME_BYTES + 1;
        let mut buf = format!("Content-Length: {len}\r\n\r\n").into_bytes();
        buf.extend_from_slice(b"partial body");
        let before = buf.len();
        assert!(parse_frames(&mut buf).is_empty());
        assert!(buf.len() < before, "oversize header was not dropped");
        // Reader moves on; a subsequent well-formed frame parses normally.
        buf.clear();
        buf.extend_from_slice(&frame("ok"));
        assert_eq!(parse_frames(&mut buf), vec!["ok".to_string()]);
    }

    #[test]
    fn parse_frames_resyncs_unframed_stream_without_unbounded_growth() {
        // F-R5-1: a stream that never contains \r\n\r\n (wrong binary usage text, a
        // server spewing garbage) must not grow the buffer without limit — parse
        // resyncs once the un-framed prefix exceeds MAX_HEADER_BYTES.
        let mut buf: Vec<u8> = Vec::new();
        let cap = MAX_HEADER_BYTES + 64 * 1024; // bound + one feed chunk
        for _ in 0..64 {
            buf.extend(std::iter::repeat_n(b'x', 64 * 1024)); // 64 KiB of garbage
            assert!(parse_frames(&mut buf).is_empty());
            // 4 MiB fed in total by the end, but the buffer stays bounded.
            assert!(
                buf.len() <= cap,
                "buffer grew unbounded on un-framed input: {}",
                buf.len()
            );
        }
        // Resync recovers: after the garbage stops, well-framed input parses again.
        buf.clear();
        buf.extend_from_slice(&frame("ok"));
        assert_eq!(parse_frames(&mut buf), vec!["ok".to_string()]);
    }

    #[test]
    fn parse_frames_scan_cost_stays_bounded_across_chunks() {
        // F-R5-1: feeding many small chunks of un-framed data must keep the buffer
        // (and therefore each header scan) bounded rather than O(n²) rescanning an
        // ever-growing buffer. We assert the invariant directly (no timing → stable).
        let mut buf: Vec<u8> = Vec::new();
        let chunk = 8 * 1024; // mirrors the reader's read size
        let cap = MAX_HEADER_BYTES + chunk;
        for _ in 0..256 {
            // 2 MiB total
            buf.extend(std::iter::repeat_n(b'z', chunk));
            assert!(parse_frames(&mut buf).is_empty());
            assert!(buf.len() <= cap, "buffer not bounded: {}", buf.len());
        }
    }

    // --- which ---

    #[test]
    fn which_finds_common_binaries() {
        assert!(which("cat").is_some());
        assert!(which("sh").is_some());
        // absolute path returned
        assert!(which("sh").unwrap().starts_with('/'));
    }

    #[test]
    fn which_missing_returns_none() {
        assert!(which("definitely-not-a-binary-xyz").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn server_bin_dirs_layout_and_priority() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join(".yuzora").join("servers");
        let dirs = server_bin_dirs_from(&base);

        // T14 landing spots in resolution order: servers root > npm private-prefix
        // bin > pylsp venv bin.
        assert_eq!(
            dirs,
            vec![
                base.clone(),
                base.join("npm").join("node_modules").join(".bin"),
                base.join("pyenv").join("bin"),
            ]
        );

        let make_exec = |dir: &Path| {
            std::fs::create_dir_all(dir).unwrap();
            let f = dir.join("srv");
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            f
        };
        let root_bin = make_exec(&dirs[0]);
        let npm_bin = make_exec(&dirs[1]);
        let pyenv_bin = make_exec(&dirs[2]);

        // First candidate wins: servers root.
        assert_eq!(
            resolve_in_dirs("srv", &dirs),
            Some(root_bin.to_string_lossy().into_owned())
        );
        // Remove root → npm private-prefix bin.
        std::fs::remove_file(&root_bin).unwrap();
        assert_eq!(
            resolve_in_dirs("srv", &dirs),
            Some(npm_bin.to_string_lossy().into_owned())
        );
        // Remove npm → pyenv venv bin.
        std::fs::remove_file(&npm_bin).unwrap();
        assert_eq!(
            resolve_in_dirs("srv", &dirs),
            Some(pyenv_bin.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn candidate_names_shapes_per_platform() {
        // F-C: Windows tries .exe/.cmd before the bare name (npm shims are
        // .cmd; the extensionless npm next to them is a Unix sh script that
        // CreateProcessW cannot spawn, #11); unix is bare.
        assert_eq!(candidate_names_for("npm", false), vec!["npm".to_string()]);
        assert_eq!(
            candidate_names_for("npm", true),
            vec![
                "npm.exe".to_string(),
                "npm.cmd".to_string(),
                "npm".to_string()
            ]
        );
        // The live wrapper picks the current platform's shape.
        #[cfg(unix)]
        assert_eq!(candidate_names("cat"), vec!["cat".to_string()]);
    }

    #[cfg(windows)]
    #[test]
    fn which_prefers_cmd_shim_over_bare_sh_script_on_windows() {
        // #11: Node's install dir ships both an extensionless `npm` (sh
        // script) and `npm.cmd`; resolution must land on the spawnable .cmd.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("npm"), "#!/bin/sh\n").unwrap();
        std::fs::write(tmp.path().join("npm.cmd"), "@echo off\r\n").unwrap();
        let resolved = resolve_in_dirs("npm", &[tmp.path().to_path_buf()]).unwrap();
        assert!(resolved.ends_with("npm.cmd"), "resolved: {resolved}");
    }
}
