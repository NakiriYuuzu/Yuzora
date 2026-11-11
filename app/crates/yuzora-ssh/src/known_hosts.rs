//! OpenSSH known_hosts files: reading, checking a presented key, Yuzora's own file.
//!
//! Reading and checking never modify a file; [`trust`] and [`remove_line`] rewrite
//! one through a temp file in the same directory, keeping every other line byte-for-byte.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::{HashAlg, parse_public_key_base64};
use serde::Serialize;
use sha1::Sha1;

use crate::config::wildcard_match;

/// One read-modify-write at a time: two hosts trusted at once both stay recorded.
static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Decodes standard base64 with or without padding.
const BASE64: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent));

/// The `@` marker in front of a known_hosts line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Marker {
    /// A plain host key.
    None,
    /// `@cert-authority`: the key signs host certificates.
    CertAuthority,
    /// `@revoked`: the key must never be accepted.
    Revoked,
}

/// One valid known_hosts line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KnownHostLine {
    /// The file it was read from.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The line as read, no newline.
    pub raw: String,
    /// The `@` marker, if any.
    pub marker: Marker,
    /// The host field: a comma list of patterns, or a `|1|salt|hash` entry.
    pub patterns: String,
    /// Whether the host field is hashed.
    pub hashed: bool,
    /// Key algorithm, e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// The base64 key blob.
    pub key: String,
    /// `SHA256:...`, as `ssh-keygen -lf` prints it.
    pub fingerprint: String,
    /// Free text after the key.
    pub comment: Option<String>,
}

/// The outcome of checking a presented host key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// An unmarked line records exactly this key for the host.
    Trusted {
        /// The first such line.
        line: KnownHostLine,
    },
    /// Same key type recorded for the host with another key.
    Changed {
        /// The lines recording the other keys.
        previous: Vec<KnownHostLine>,
    },
    /// An `@revoked` line names this key.
    Revoked {
        /// The first such line.
        line: KnownHostLine,
    },
    /// Nothing for this key type; `other_algorithms` recorded for the host (may be empty).
    Unknown {
        /// Algorithms of the other keys recorded for the host, in order of appearance.
        other_algorithms: Vec<String>,
    },
}

/// Every valid line; a missing file reads as none, malformed lines are skipped.
pub fn read_file(path: &Path) -> Vec<KnownHostLine> {
    raw_lines(path).into_iter().filter_map(|(no, raw)| parse_line(path, no, &raw)).collect()
}

/// The name looked up: "host" on port 22, "[host]:port" otherwise.
pub fn lookup_name(host: &str, port: u16) -> String {
    // ssh lowercases the host before looking it up (and before hashing it).
    let host = host.to_ascii_lowercase();
    if port == 22 { host } else { format!("[{host}]:{port}") }
}

/// Does a line's host field match? Comma list; "|1|salt|hash" (HMAC-SHA1 of lookup_name, base64);
/// * and ? wildcards; "!pattern" negation (a negated match means no match); plain names case-insensitive.
pub fn host_matches(patterns: &str, host: &str, port: u16) -> bool {
    let name = lookup_name(host, port);
    if patterns.starts_with('|') {
        return patterns.strip_prefix("|1|").is_some_and(|hashed| hashed_matches(hashed, &name));
    }
    let mut hit = false;
    for pattern in patterns.split(',').filter(|p| !p.is_empty()) {
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern),
        };
        if wildcard_match(&pattern.to_ascii_lowercase(), &name) {
            if negated {
                return false;
            }
            hit = true;
        }
    }
    hit
}

/// Checks the presented key against every file in order (later files only add lines).
/// Revoked (an @revoked line matching the host with this key) beats everything; then Trusted
/// if any unmarked matching line has this algorithm+key; then Changed; else Unknown.
/// @cert-authority lines are ignored. Keys compare by decoded blob.
pub fn check(files: &[PathBuf], host: &str, port: u16, algorithm: &str, key: &str) -> Verdict {
    let Some(presented) = decode(key) else {
        return Verdict::Unknown { other_algorithms: Vec::new() };
    };
    let (mut revoked, mut trusted) = (None, None);
    let (mut changed, mut others): (Vec<KnownHostLine>, Vec<String>) = (Vec::new(), Vec::new());
    for file in files {
        for (no, raw) in raw_lines(file) {
            // Only lines for this host are worth parsing the key of.
            if !split_fields(&raw).is_some_and(|f| host_matches(f.patterns, host, port)) {
                continue;
            }
            let Some(line) = parse_line(file, no, &raw) else { continue };
            let same_key = decode(&line.key).as_deref() == Some(presented.as_slice());
            match line.marker {
                Marker::CertAuthority => {}
                Marker::Revoked => {
                    if same_key && revoked.is_none() {
                        revoked = Some(line);
                    }
                }
                Marker::None if line.algorithm == algorithm => {
                    if !same_key {
                        changed.push(line);
                    } else if trusted.is_none() {
                        trusted = Some(line);
                    }
                }
                Marker::None => {
                    if !others.contains(&line.algorithm) {
                        others.push(line.algorithm);
                    }
                }
            }
        }
    }
    if let Some(line) = revoked {
        Verdict::Revoked { line }
    } else if let Some(line) = trusted {
        Verdict::Trusted { line }
    } else if !changed.is_empty() {
        Verdict::Changed { previous: changed }
    } else {
        Verdict::Unknown { other_algorithms: others }
    }
}

/// Records a key in `path` (Yuzora's own file): drops that file's unmarked lines for the
/// same lookup name and algorithm, then appends "lookup_name algorithm key". Creates parent
/// dirs; 0600 on unix; write to a temp file in the same dir and rename; other lines kept byte-for-byte.
pub fn trust(path: &Path, host: &str, port: u16, algorithm: &str, key: &str) -> io::Result<()> {
    let _one_writer = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let name = lookup_name(host, port);
    for field in [name.as_str(), algorithm, key] {
        if field.is_empty() || field.contains(|c: char| c.is_whitespace() || c.is_control()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "known_hosts fields cannot be empty or contain whitespace"));
        }
    }
    let old = read_or_empty(path)?;
    let mut out = Vec::with_capacity(old.len() + name.len() + algorithm.len() + key.len() + 3);
    for chunk in old.split_inclusive(|b| *b == b'\n') {
        if !records_same_entry(&line_text(chunk), &name, algorithm) {
            out.extend_from_slice(chunk);
        }
    }
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(format!("{name} {algorithm} {key}\n").as_bytes());
    write_atomic(path, &out, 0o600)
}

/// Removes line `line` only if it still reads `raw`; same temp+rename. Ok(false) when it changed.
pub fn remove_line(path: &Path, line: usize, raw: &str) -> io::Result<bool> {
    let _one_writer = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let old = read_or_empty(path)?;
    let mut out = Vec::with_capacity(old.len());
    let mut removed = false;
    for (i, chunk) in old.split_inclusive(|b| *b == b'\n').enumerate() {
        if i + 1 == line && line_text(chunk) == raw {
            removed = true;
        } else {
            out.extend_from_slice(chunk);
        }
    }
    if !removed {
        return Ok(false);
    }
    write_atomic(path, &out, existing_mode(path))?;
    Ok(true)
}

/// "SHA256:<base64 no padding>" of a base64 key blob (None if not valid base64).
///
/// Also None when the blob is not a public key this build can parse; such lines are not read.
pub fn fingerprint(key: &str) -> Option<String> {
    Some(parse_public_key_base64(key).ok()?.fingerprint(HashAlg::Sha256).to_string())
}

/// A line's whitespace-separated fields, borrowed from its text.
struct Fields<'a> {
    marker: Marker,
    patterns: &'a str,
    algorithm: &'a str,
    key: &'a str,
    comment: Option<&'a str>,
}

fn next_word(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches([' ', '\t']);
    match s.find([' ', '\t']) {
        Some(i) => s.split_at(i),
        None => (s, ""),
    }
}

/// `[@marker ]patterns algorithm key [comment]`; None for comments, blanks and short lines.
fn split_fields(line: &str) -> Option<Fields<'_>> {
    let rest = line.trim_start_matches([' ', '\t']);
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    let (marker, rest) = if rest.starts_with('@') {
        let (word, tail) = next_word(rest);
        let marker = match word {
            "@cert-authority" => Marker::CertAuthority,
            "@revoked" => Marker::Revoked,
            _ => return None,
        };
        (marker, tail)
    } else {
        (Marker::None, rest)
    };
    let (patterns, rest) = next_word(rest);
    let (algorithm, rest) = next_word(rest);
    let (key, rest) = next_word(rest);
    if patterns.is_empty() || algorithm.is_empty() || key.is_empty() {
        return None;
    }
    let comment = Some(rest.trim()).filter(|c| !c.is_empty());
    Some(Fields { marker, patterns, algorithm, key, comment })
}

/// A valid line: well-formed fields and a key that parses and agrees with its algorithm.
fn parse_line(file: &Path, line: usize, raw: &str) -> Option<KnownHostLine> {
    let f = split_fields(raw)?;
    let parsed = parse_public_key_base64(f.key).ok()?;
    if parsed.algorithm().as_str() != f.algorithm {
        return None;
    }
    Some(KnownHostLine {
        file: file.to_path_buf(),
        line,
        raw: raw.to_string(),
        marker: f.marker,
        patterns: f.patterns.to_string(),
        hashed: f.patterns.starts_with('|'),
        algorithm: f.algorithm.to_string(),
        key: f.key.to_string(),
        fingerprint: parsed.fingerprint(HashAlg::Sha256).to_string(),
        comment: f.comment.map(String::from),
    })
}

/// `(1-based number, text)` for every line of the file; unreadable reads as empty.
fn raw_lines(path: &Path) -> Vec<(usize, String)> {
    let Ok(bytes) = fs::read(path) else { return Vec::new() };
    bytes.split_inclusive(|b| *b == b'\n').enumerate().map(|(i, chunk)| (i + 1, line_text(chunk))).collect()
}

/// A line without its `\n` / `\r\n`.
fn line_text(chunk: &[u8]) -> String {
    let chunk = chunk.strip_suffix(b"\n").unwrap_or(chunk);
    let chunk = chunk.strip_suffix(b"\r").unwrap_or(chunk);
    String::from_utf8_lossy(chunk).into_owned()
}

fn decode(s: &str) -> Option<Vec<u8>> {
    BASE64.decode(s).ok()
}

/// `salt|hash` (the part after `|1|`): HMAC-SHA1 of `name` keyed by the salt.
fn hashed_matches(entry: &str, name: &str) -> bool {
    let Some((salt, hash)) = entry.split_once('|') else { return false };
    let (Some(salt), Some(hash)) = (decode(salt), decode(hash)) else { return false };
    Hmac::<Sha1>::new_from_slice(&salt).is_ok_and(|mac| mac.chain_update(name.as_bytes()).verify_slice(&hash).is_ok())
}

/// Is `text` an unmarked line for exactly this lookup name and algorithm (what [`trust`] replaces)?
fn records_same_entry(text: &str, name: &str, algorithm: &str) -> bool {
    let Some(f) = split_fields(text) else { return false };
    if f.marker != Marker::None || f.algorithm != algorithm {
        return false;
    }
    match f.patterns.strip_prefix("|1|") {
        Some(hashed) => hashed_matches(hashed, name),
        None => !f.patterns.contains(',') && f.patterns.eq_ignore_ascii_case(name),
    }
}

fn read_or_empty(path: &Path) -> io::Result<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn existing_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600)
}

#[cfg(not(unix))]
fn existing_mode(_path: &Path) -> u32 {
    0o600
}

/// Writes `data` to a temp file next to `path` (created with `mode` on unix) and renames it over.
fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let Some(name) = path.file_name() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "known_hosts path has no file name"));
    };
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.{}.tmp", name.to_string_lossy(), std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let written = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway keys made with `ssh-keygen -t <type>`; the fingerprints are what `ssh-keygen -lf` printed.
    const ED_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOus8jPau8gnbpPTvouyRvcckZ4kVeTbopqk2HwyPU9h";
    const ED_A_FP: &str = "SHA256:NPjCCNykupfxXQpI3ZiK6QpIsvn3+h4HPKDlTX/4Z9A";
    const ED_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIH1V/AswXEF8kP3UIGyX789XXA3jRL9/mCihLkTvswU7";
    const ED_B_FP: &str = "SHA256:MGIJhlkXcn7S6EVdXflq20Z2SzcazQrLJWrjYCpXwdI";
    const RSA_C: &str = "AAAAB3NzaC1yc2EAAAADAQABAAABAQDcDYDvIYBSjSLnSRkYj//CUNWNHbIEMPGwx4wKOVudYwfZ1f2BrjWNEK1o93A3ae8VnVQqHZLRTLsIPzvXdBcxLZRuQEMSkO6NcHVnA21C1FAJIi1GMX9nOky7x3oM/a+wCX4bStBvuRs9FF5Cc75upOnU9CeI2ubXsl4BowQq7f5lrD+/fpt/DYo2HFpUaxrVkFlrQasSjZuRPOSenfB3kB/dKukgM9HsZ9ahADnItiPjXge+3yP5gadwMMbCvywvnLb1uSWEVPdHfnzGTrWCRpk9bp0Vuvm4uf6DDVqRMVIs5b1mE2x3phOJJGH/nIjQe3qluvwqRvAe7vPFpYnZ";
    const RSA_C_FP: &str = "SHA256:/n1TNPjeAXstARiTUSVuBKzPMTUenYUyFj/bUGncIvo";
    const EC_D: &str = "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBOcixOu5pi9fD0WBMx54a2mc8xJ8nJGfBNJuP9H8Bz092m3IneD2+CBH0VniefA9ZMAVunk+t7JD6u4SA4PTXVg=";
    const EC_D_FP: &str = "SHA256:of4gfc+DE+hirobRH7j1Um3lPwfNdCop5DUf9oCyt2Y";
    // `ssh-keygen -H` of "hashed.example.com" and "[hashed.example.com]:2222" with ED_A.
    const HASHED_22: &str = "|1|YC9rw+0dpmlkVk+K/okRJflioJM=|rlbH92gLs3FXLqJsDIycKCWye04=";
    const HASHED_2222: &str = "|1|wF2qRDzkkmrDRWewzcaarCldJkc=|LvdKpzQ1QgtRJya+03MR7GV/7lU=";

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("yuzora-ssh-test-{}-kh-{}", std::process::id(), name));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn fingerprints_match_ssh_keygen() {
        assert_eq!(fingerprint(ED_A).as_deref(), Some(ED_A_FP));
        assert_eq!(fingerprint(ED_B).as_deref(), Some(ED_B_FP));
        assert_eq!(fingerprint(RSA_C).as_deref(), Some(RSA_C_FP));
        assert_eq!(fingerprint(EC_D).as_deref(), Some(EC_D_FP));
        assert_eq!(fingerprint("not base64!"), None);
        assert_eq!(fingerprint("AAAA"), None);
        assert_eq!(fingerprint(""), None);
    }

    #[test]
    fn lookup_name_brackets_non_default_ports() {
        assert_eq!(lookup_name("example.com", 22), "example.com");
        assert_eq!(lookup_name("example.com", 2222), "[example.com]:2222");
        assert_eq!(lookup_name("Example.COM", 22), "example.com");
        assert_eq!(lookup_name("::1", 22), "::1");
        assert_eq!(lookup_name("::1", 2200), "[::1]:2200");
    }

    #[test]
    fn plain_patterns_lists_wildcards_and_ports() {
        assert!(host_matches("example.com", "example.com", 22));
        assert!(host_matches("EXAMPLE.com", "Example.COM", 22));
        assert!(!host_matches("example.com", "example.com", 2222));
        assert!(host_matches("[example.com]:2222", "example.com", 2222));
        assert!(!host_matches("[example.com]:2222", "example.com", 22));
        assert!(host_matches("a.example.com,b.example.com,10.0.0.1", "b.example.com", 22));
        assert!(host_matches("a,10.0.0.1", "10.0.0.1", 22));
        assert!(host_matches("*.example.com", "x.example.com", 22));
        assert!(!host_matches("*.example.com", "example.org", 22));
        assert!(host_matches("host?.example.com", "host1.example.com", 22));
        assert!(!host_matches("host?.example.com", "host12.example.com", 22));
        assert!(host_matches("[*.example.com]:2222", "x.example.com", 2222));
        assert!(!host_matches("*.example.com", "x.example.com", 2222), "unbracketed patterns only cover port 22");
        assert!(host_matches("[::1]:2200", "::1", 2200));
        assert!(host_matches("::1", "::1", 22));
        assert!(!host_matches("", "example.com", 22));
    }

    #[test]
    fn negation_beats_a_positive_match() {
        assert!(host_matches("*.example.com,!bad.example.com", "good.example.com", 22));
        assert!(!host_matches("*.example.com,!bad.example.com", "bad.example.com", 22));
        assert!(!host_matches("!bad.example.com,*.example.com", "bad.example.com", 22), "order does not matter");
        assert!(!host_matches("!only.example.com", "other.example.com", 22), "a negation alone matches nothing");
        assert!(!host_matches("!only.example.com", "only.example.com", 22));
    }

    #[test]
    fn hashed_entries_match_ssh_keygen_output() {
        assert!(host_matches(HASHED_22, "hashed.example.com", 22));
        assert!(host_matches(HASHED_22, "Hashed.Example.COM", 22));
        assert!(!host_matches(HASHED_22, "other.example.com", 22));
        assert!(!host_matches(HASHED_22, "hashed.example.com", 2222));
        assert!(host_matches(HASHED_2222, "hashed.example.com", 2222));
        assert!(!host_matches(HASHED_2222, "hashed.example.com", 22));
    }

    #[test]
    fn hashed_entries_computed_here_round_trip() {
        let salt = [7u8; 20];
        let mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap().chain_update(b"[box.internal]:2022").finalize().into_bytes();
        let entry = format!("|1|{}|{}", base64::engine::general_purpose::STANDARD.encode(salt), base64::engine::general_purpose::STANDARD.encode(mac));
        assert!(host_matches(&entry, "box.internal", 2022));
        assert!(!host_matches(&entry, "box.internal", 22));
    }

    #[test]
    fn malformed_hashed_entries_never_match() {
        for bad in ["|1|", "|1|abc", "|1|!!!|!!!", "|2|YC9rw+0dpmlkVk+K/okRJflioJM=|rlbH92gLs3FXLqJsDIycKCWye04=", "|", "|1|YC9rw+0dpmlkVk+K/okRJflioJM=|AAAA"] {
            assert!(!host_matches(bad, "hashed.example.com", 22), "{bad}");
        }
    }

    #[test]
    fn read_file_returns_valid_lines_with_numbers_and_details() {
        let t = TempDir::new("read");
        let text = format!(
            "# a comment\n\nexample.com ssh-ed25519 {ED_A} work laptop\n@cert-authority *.example.com ssh-rsa {RSA_C}\n@revoked [bad.example.com]:2222 ecdsa-sha2-nistp256 {EC_D}\n  \tspaced.example.com\tssh-ed25519\t{ED_B}\r\n{HASHED_22} ssh-ed25519 {ED_A}\nbroken.example.com ssh-ed25519 !!!notbase64!!!\nmismatch.example.com ssh-rsa {ED_A}\n@unknown example.com ssh-ed25519 {ED_A}\nshort.example.com ssh-ed25519\nold.example.com 1024 35 1234567890\n"
        );
        let path = t.write("known_hosts", &text);
        let lines = read_file(&path);
        let numbers: Vec<usize> = lines.iter().map(|l| l.line).collect();
        assert_eq!(numbers, [3, 4, 5, 6, 7]);
        let first = &lines[0];
        assert_eq!(first.file, path);
        assert_eq!(first.raw, format!("example.com ssh-ed25519 {ED_A} work laptop"));
        assert_eq!((first.marker, first.patterns.as_str(), first.hashed), (Marker::None, "example.com", false));
        assert_eq!((first.algorithm.as_str(), first.key.as_str(), first.fingerprint.as_str()), ("ssh-ed25519", ED_A, ED_A_FP));
        assert_eq!(first.comment.as_deref(), Some("work laptop"));
        assert_eq!((lines[1].marker, lines[1].patterns.as_str(), lines[1].comment.as_deref()), (Marker::CertAuthority, "*.example.com", None));
        assert_eq!(lines[1].fingerprint, RSA_C_FP);
        assert_eq!((lines[2].marker, lines[2].patterns.as_str()), (Marker::Revoked, "[bad.example.com]:2222"));
        assert_eq!(lines[3].raw, format!("  \tspaced.example.com\tssh-ed25519\t{ED_B}"), "CRLF is not part of raw");
        assert_eq!(lines[3].patterns, "spaced.example.com");
        assert!(lines[4].hashed);
        assert_eq!(lines[4].patterns, HASHED_22);
    }

    #[test]
    fn missing_or_empty_files_read_as_none() {
        let t = TempDir::new("read-missing");
        assert!(read_file(&t.0.join("nope")).is_empty());
        assert!(read_file(&t.write("empty", "")).is_empty());
        assert!(read_file(&t.0).is_empty(), "a directory reads as none");
    }

    #[test]
    fn check_trusted_changed_and_unknown() {
        let t = TempDir::new("check-basic");
        let path = t.write("kh", &format!("example.com ssh-ed25519 {ED_A}\nexample.com ssh-rsa {RSA_C}\nother.example.com ssh-ed25519 {ED_B}\n"));
        let files = [path];
        match check(&files, "example.com", 22, "ssh-ed25519", ED_A) {
            Verdict::Trusted { line } => assert_eq!((line.line, line.fingerprint.as_str()), (1, ED_A_FP)),
            other => panic!("{other:?}"),
        }
        match check(&files, "example.com", 22, "ssh-ed25519", ED_B) {
            Verdict::Changed { previous } => assert_eq!(previous.iter().map(|l| l.line).collect::<Vec<_>>(), [1]),
            other => panic!("{other:?}"),
        }
        assert_eq!(check(&files, "example.com", 22, "ecdsa-sha2-nistp256", EC_D), Verdict::Unknown { other_algorithms: vec!["ssh-ed25519".into(), "ssh-rsa".into()] });
        assert_eq!(check(&files, "never-seen.example.com", 22, "ssh-ed25519", ED_A), Verdict::Unknown { other_algorithms: vec![] });
        // The same key under another host name does not count.
        assert_eq!(check(&files, "other.example.com", 22, "ssh-ed25519", ED_A), Verdict::Changed { previous: read_file(&files[0]).into_iter().skip(2).collect() });
        // Another port is another host.
        assert_eq!(check(&files, "example.com", 2222, "ssh-ed25519", ED_A), Verdict::Unknown { other_algorithms: vec![] });
    }

    #[test]
    fn check_with_an_undecodable_key_is_unknown() {
        let t = TempDir::new("check-undecodable");
        let files = [t.write("kh", &format!("example.com ssh-rsa {RSA_C}\n"))];
        assert!(matches!(check(&files, "example.com", 22, "ssh-rsa", RSA_C), Verdict::Trusted { .. }));
        assert_eq!(check(&files, "example.com", 22, "ssh-rsa", "!!!"), Verdict::Unknown { other_algorithms: vec![] });
    }

    #[test]
    fn check_handles_ports_wildcards_negation_and_hashed_lines() {
        let t = TempDir::new("check-patterns");
        let text = format!(
            "[example.com]:2222 ssh-ed25519 {ED_A}\n*.lab.example.com,!secret.lab.example.com ssh-ed25519 {ED_B}\n{HASHED_22} ssh-ed25519 {ED_A}\n{HASHED_2222} ssh-ed25519 {ED_B}\n"
        );
        let files = [t.write("kh", &text)];
        assert!(matches!(check(&files, "example.com", 2222, "ssh-ed25519", ED_A), Verdict::Trusted { .. }));
        assert!(matches!(check(&files, "example.com", 22, "ssh-ed25519", ED_A), Verdict::Unknown { .. }));
        assert!(matches!(check(&files, "box.lab.example.com", 22, "ssh-ed25519", ED_B), Verdict::Trusted { .. }));
        assert!(matches!(check(&files, "secret.lab.example.com", 22, "ssh-ed25519", ED_B), Verdict::Unknown { .. }));
        assert!(matches!(check(&files, "hashed.example.com", 22, "ssh-ed25519", ED_A), Verdict::Trusted { .. }));
        assert!(matches!(check(&files, "hashed.example.com", 22, "ssh-ed25519", ED_B), Verdict::Changed { .. }));
        assert!(matches!(check(&files, "hashed.example.com", 2222, "ssh-ed25519", ED_B), Verdict::Trusted { .. }));
    }

    #[test]
    fn revoked_beats_trusted_and_cert_authority_is_ignored() {
        let t = TempDir::new("check-marks");
        let user = t.write("user", &format!("example.com ssh-ed25519 {ED_A}\nexample.com ssh-ed25519 {ED_B}\n"));
        let global = t.write("global", &format!("@revoked * ssh-ed25519 {ED_A}\n@cert-authority *.example.com ssh-ed25519 {ED_B}\n@cert-authority example.com ssh-rsa {RSA_C}\n"));
        let files = [user, global];
        match check(&files, "example.com", 22, "ssh-ed25519", ED_A) {
            Verdict::Revoked { line } => assert_eq!((line.line, line.marker), (1, Marker::Revoked)),
            other => panic!("{other:?}"),
        }
        // A key the revoked line does not name is still trusted.
        assert!(matches!(check(&files, "example.com", 22, "ssh-ed25519", ED_B), Verdict::Trusted { .. }));
        // CA lines neither trust nor count as another algorithm.
        assert_eq!(check(&files, "example.com", 22, "ssh-rsa", RSA_C), Verdict::Unknown { other_algorithms: vec!["ssh-ed25519".into()] });
        let only_ca = [t.write("ca", &format!("@cert-authority example.com ssh-ed25519 {ED_A}\n"))];
        assert_eq!(check(&only_ca, "example.com", 22, "ssh-ed25519", ED_A), Verdict::Unknown { other_algorithms: vec![] });
        // A revoked line for a different host does not apply.
        let elsewhere = [t.write("else", &format!("@revoked other.example.com ssh-ed25519 {ED_A}\nexample.com ssh-ed25519 {ED_A}\n"))];
        assert!(matches!(check(&elsewhere, "example.com", 22, "ssh-ed25519", ED_A), Verdict::Trusted { .. }));
    }

    #[test]
    fn later_files_only_add_lines() {
        let t = TempDir::new("check-files");
        let first = t.write("first", &format!("example.com ssh-ed25519 {ED_A}\n"));
        let second = t.write("second", &format!("example.com ssh-ed25519 {ED_B}\nexample.com ecdsa-sha2-nistp256 {EC_D}\n"));
        let missing = t.0.join("missing");
        let files = [first, missing, second];
        match check(&files, "example.com", 22, "ssh-ed25519", ED_B) {
            Verdict::Trusted { line } => assert!(line.file.ends_with("second")),
            other => panic!("{other:?}"),
        }
        match check(&files, "example.com", 22, "ssh-ed25519", ED_A) {
            Verdict::Trusted { line } => assert!(line.file.ends_with("first")),
            other => panic!("{other:?}"),
        }
        match check(&files, "example.com", 22, "ssh-rsa", RSA_C) {
            Verdict::Unknown { other_algorithms } => assert_eq!(other_algorithms, ["ssh-ed25519", "ecdsa-sha2-nistp256"]),
            other => panic!("{other:?}"),
        }
        assert_eq!(check(&[], "example.com", 22, "ssh-ed25519", ED_A), Verdict::Unknown { other_algorithms: vec![] });
    }

    #[test]
    fn trust_creates_the_file_and_parents_privately() {
        let t = TempDir::new("trust-new");
        let path = t.0.join("nested/dir/known_hosts");
        trust(&path, "Example.com", 22, "ssh-ed25519", ED_A).unwrap();
        assert_eq!(read(&path), format!("example.com ssh-ed25519 {ED_A}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(matches!(check(&[path], "example.com", 22, "ssh-ed25519", ED_A), Verdict::Trusted { .. }));
    }

    #[test]
    fn trust_replaces_the_same_name_and_algorithm_and_keeps_everything_else() {
        let t = TempDir::new("trust-replace");
        let original = format!(
            "# mine\nexample.com ssh-ed25519 {ED_A} old\nexample.com ssh-rsa {RSA_C}\n[example.com]:2222 ssh-ed25519 {ED_A}\r\nother.com ssh-ed25519 {ED_A}\n@revoked example.com ssh-ed25519 {ED_B}\nEXAMPLE.COM ssh-ed25519 {ED_B}\n{HASHED_22} ssh-ed25519 {ED_A}\na.com,example.com ssh-ed25519 {ED_A}\nbroken line here\n"
        );
        let path = t.write("kh", &original);
        trust(&path, "example.com", 22, "ssh-ed25519", ED_B).unwrap();
        let expected = format!(
            "# mine\nexample.com ssh-rsa {RSA_C}\n[example.com]:2222 ssh-ed25519 {ED_A}\r\nother.com ssh-ed25519 {ED_A}\n@revoked example.com ssh-ed25519 {ED_B}\n{HASHED_22} ssh-ed25519 {ED_A}\na.com,example.com ssh-ed25519 {ED_A}\nbroken line here\nexample.com ssh-ed25519 {ED_B}\n"
        );
        assert_eq!(read(&path), expected);
        // A non-default port is its own name.
        trust(&path, "example.com", 2222, "ssh-ed25519", ED_B).unwrap();
        assert!(read(&path).contains(&format!("[example.com]:2222 ssh-ed25519 {ED_B}\n")));
        assert!(!read(&path).contains(&format!("[example.com]:2222 ssh-ed25519 {ED_A}")));
        assert!(read(&path).contains(&format!("example.com ssh-ed25519 {ED_B}\n")));
    }

    #[test]
    fn trust_drops_hashed_lines_for_the_same_host() {
        let t = TempDir::new("trust-hashed");
        let path = t.write("kh", &format!("{HASHED_22} ssh-ed25519 {ED_A}\n{HASHED_2222} ssh-ed25519 {ED_A}\n"));
        trust(&path, "hashed.example.com", 22, "ssh-ed25519", ED_B).unwrap();
        assert_eq!(read(&path), format!("{HASHED_2222} ssh-ed25519 {ED_A}\nhashed.example.com ssh-ed25519 {ED_B}\n"));
    }

    #[test]
    fn trust_adds_the_missing_final_newline() {
        let t = TempDir::new("trust-newline");
        let path = t.write("kh", &format!("other.com ssh-ed25519 {ED_A}"));
        trust(&path, "example.com", 22, "ssh-ed25519", ED_B).unwrap();
        assert_eq!(read(&path), format!("other.com ssh-ed25519 {ED_A}\nexample.com ssh-ed25519 {ED_B}\n"));
    }

    #[test]
    fn trust_narrows_a_loose_existing_file_and_leaves_no_temp_files() {
        let t = TempDir::new("trust-mode");
        let path = t.write("kh", "");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        trust(&path, "example.com", 22, "ssh-ed25519", ED_A).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let names: Vec<_> = fs::read_dir(&t.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["kh"]);
    }

    #[test]
    fn trust_rejects_fields_that_would_corrupt_the_file() {
        let t = TempDir::new("trust-invalid");
        let path = t.0.join("kh");
        for (host, algorithm, key) in [("ex ample.com", "ssh-ed25519", ED_A), ("example.com\n@revoked *", "ssh-ed25519", ED_A), ("example.com", "ssh-ed25519 x", ED_A), ("example.com", "ssh-ed25519", "AAAA\nBBBB"), ("", "ssh-ed25519", ED_A), ("example.com", "", ED_A)] {
            let err = trust(&path, host, 22, algorithm, key).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{host:?} {algorithm:?}");
        }
        assert!(!path.exists());
    }

    #[test]
    fn trust_fails_when_the_parent_is_a_file() {
        let t = TempDir::new("trust-blocked");
        let blocker = t.write("blocker", "x");
        assert!(trust(&blocker.join("kh"), "example.com", 22, "ssh-ed25519", ED_A).is_err());
    }

    #[test]
    fn remove_line_only_removes_an_unchanged_line() {
        let t = TempDir::new("remove");
        let l1 = format!("one.com ssh-ed25519 {ED_A}");
        let l2 = format!("two.com ssh-ed25519 {ED_B}");
        let l3 = format!("three.com ssh-rsa {RSA_C}");
        let path = t.write("kh", &format!("{l1}\r\n{l2}\n# note\n{l3}"));
        // Stale content or a wrong number removes nothing.
        assert!(!remove_line(&path, 2, &l1).unwrap());
        assert!(!remove_line(&path, 1, &l2).unwrap());
        assert!(!remove_line(&path, 0, &l1).unwrap());
        assert!(!remove_line(&path, 9, &l1).unwrap());
        assert_eq!(read(&path), format!("{l1}\r\n{l2}\n# note\n{l3}"));
        // The matching line goes; the rest is untouched, CRLF and missing final newline included.
        assert!(remove_line(&path, 2, &l2).unwrap());
        assert_eq!(read(&path), format!("{l1}\r\n# note\n{l3}"));
        assert!(remove_line(&path, 3, &l3).unwrap());
        assert_eq!(read(&path), format!("{l1}\r\n# note\n"));
        assert!(remove_line(&path, 1, &l1).unwrap());
        assert_eq!(read(&path), "# note\n");
        assert!(!remove_line(&t.0.join("nope"), 1, &l1).unwrap());
    }

    #[test]
    fn remove_line_works_on_what_read_file_returns_and_keeps_the_mode() {
        let t = TempDir::new("remove-read");
        let path = t.write("kh", &format!("a.com ssh-ed25519 {ED_A} x\nb.com ssh-ed25519 {ED_B}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let line = read_file(&path).remove(0);
        assert!(remove_line(&path, line.line, &line.raw).unwrap());
        assert_eq!(read(&path), format!("b.com ssh-ed25519 {ED_B}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
        }
    }
}
