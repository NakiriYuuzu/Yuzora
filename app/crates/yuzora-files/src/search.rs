//! Search for the Files lane: the file list for quick open, fuzzy matching of
//! paths, and content search through ripgrep. Blocking. Here or on a
//! herdr-core machine (`on: Some(machine)`, POSIX paths).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use yuzora_core::remote::{self, ExecError, ExecRequest};

use crate::fsops::{failure, machine, run_sh};

const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_MAX_OUTPUT: u64 = 32 << 20;
const GREP_TIMEOUT: Duration = Duration::from_secs(20);
const GREP_MAX_OUTPUT: u64 = 8 << 20;
const RG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// A hit's line is cut to this many characters.
const TEXT_CHARS: usize = 300;

/// The files under `$1` (relative): git's list in a work tree, else
/// ripgrep's, else find's. `$2` is 1 to include hidden files. Prints the
/// tool's name, then NUL-terminated paths; git's are followed by an empty
/// entry and the tracked files deleted from the work tree. find knows no
/// ignore files.
const LIST_SCRIPT: &str = r#"cd -- "$1" 2>/dev/null || { echo "cannot open $1" >&2; exit 3; }
g() { git --no-optional-locks -c core.fsmonitor=false "$@"; }
if command -v git >/dev/null 2>&1 && [ "$(g rev-parse --is-inside-work-tree 2>/dev/null)" = true ]; then
  printf 'git\0'
  g ls-files -co --exclude-standard -z || exit $?
  printf '\0'
  g ls-files -d -z; exit $?
fi
if command -v rg >/dev/null 2>&1; then
  printf 'rg\0'
  if [ "$2" = 1 ]; then exec rg --files --null --hidden -g '!.git'; fi
  exec rg --files --null -g '!.git'
fi
printf 'find\0'
exec find . -type f -not -path '*/.git/*' -print0"#;

// ---- the file list ----

/// The files found under a folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileList {
    /// Relative, `/`-separated, sorted.
    pub files: Vec<String>,
    /// There were more than the limit.
    pub truncated: bool,
    /// Ignore files were not applied: the machine has neither git nor ripgrep.
    pub unfiltered: bool,
}

/// Files under `root` (relative, `/`-separated), honoring `.gitignore` and
/// `.ignore` (see [`FileList::unfiltered`]), never `.git`; hidden files only
/// when `hidden`. At most `limit`.
pub fn list_files(
    root: &str,
    on: Option<&str>,
    hidden: bool,
    limit: usize,
) -> Result<FileList, String> {
    match machine(on) {
        Some(m) => list_sh(root, Some(m), hidden, limit),
        None => list_local(root, hidden, limit),
    }
}

fn list_local(root: &str, hidden: bool, limit: usize) -> Result<FileList, String> {
    let base = Path::new(root);
    if !base.is_dir() {
        return Err(format!("cannot read {root}: not a folder"));
    }
    let walk = ignore::WalkBuilder::new(base)
        .hidden(!hidden)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    let mut files = Vec::new();
    for entry in walk.flatten() {
        // A symlink to a file counts; a link to a folder is not followed.
        let file = entry.file_type().is_some_and(|t| {
            t.is_file() || (t.is_symlink() && entry.path().metadata().is_ok_and(|m| m.is_file()))
        });
        let Ok(rel) = entry.path().strip_prefix(base) else {
            continue;
        };
        if !file || rel.as_os_str().is_empty() {
            continue;
        }
        if files.len() >= limit {
            return Ok(FileList {
                files,
                truncated: true,
                unfiltered: false,
            });
        }
        files.push(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    Ok(FileList {
        files,
        ..FileList::default()
    })
}

fn list_sh(root: &str, on: Option<&str>, hidden: bool, limit: usize) -> Result<FileList, String> {
    let out = run_sh(
        on,
        LIST_SCRIPT,
        &[root, if hidden { "1" } else { "0" }],
        LIST_TIMEOUT,
        LIST_MAX_OUTPUT,
    )?;
    // find and rg exit non-zero for an unreadable corner or an empty tree and
    // still print what they found.
    let listed = out
        .stdout
        .iter()
        .position(|b| *b == 0)
        .is_some_and(|tag| tag + 1 < out.stdout.len());
    if !listed && !matches!(out.code, 0 | 1) {
        return Err(failure(&out, "listing the files"));
    }
    let capped = out.stdout.len() as u64 >= LIST_MAX_OUTPUT;
    let mut bytes = &out.stdout[..];
    if capped && let Some(nul) = bytes.iter().rposition(|b| *b == 0) {
        bytes = &bytes[..=nul];
    }
    let mut list = parse_file_list(bytes, hidden, limit);
    list.truncated |= capped;
    Ok(list)
}

/// What [`LIST_SCRIPT`] prints: the tool, then paths with `./` dropped,
/// git's deleted files, `.git` and (unless `hidden`) dot-files left out;
/// sorted, cut to `limit`.
fn parse_file_list(out: &[u8], hidden: bool, limit: usize) -> FileList {
    let mut entries = out.split(|b| *b == 0);
    let tool = entries.next().unwrap_or_default();
    let listed: Vec<&[u8]> = entries.by_ref().take_while(|p| !p.is_empty()).collect();
    let gone: HashSet<&[u8]> = entries.filter(|p| !p.is_empty()).collect();
    let mut files: Vec<String> = listed
        .into_iter()
        .filter(|p| !gone.contains(p))
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .map(|p| {
            p.strip_prefix("./")
                .map_or_else(|| p.clone(), str::to_owned)
        })
        .filter(|p| {
            p.split('/')
                .all(|part| part != ".git" && (hidden || !part.starts_with('.')))
        })
        .collect();
    files.sort();
    let truncated = files.len() > limit;
    files.truncate(limit);
    FileList {
        files,
        truncated,
        unfiltered: tool == b"find",
    }
}

// ---- fuzzy matching ----

const SCORE_MATCH: i64 = 16;
const GAP_START: i64 = 3;
const GAP_EXTEND: i64 = 1;
const BONUS_SLASH: i64 = 9;
const BONUS_BOUNDARY: i64 = 8;
const BONUS_CAMEL: i64 = 7;
const BONUS_CONSECUTIVE: i64 = 6;
const BONUS_BASENAME: i64 = 5;
const BONUS_PREFIX: i64 = 6;
const NONE: i64 = i64::MIN / 4;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct FuzzyMatch {
    pub path: String,
    pub score: i64,
    /// Character positions in `path` that matched.
    pub indices: Vec<usize>,
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Subsequence match of `query` in `candidate`, best alignment by score.
/// Case-insensitive, except that an uppercase query character must match
/// exactly. Whitespace in the query is ignored. Bonuses: a match in the
/// basename, a run of consecutive matches, a word start (after `/ _ - .` or a
/// space, or a lower-to-upper camel hump), the very start of the path;
/// penalties: gaps and a long path. `None` when it is not a subsequence.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<(i64, Vec<usize>)> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let m = q.len();
    let qf: Vec<char> = q.iter().map(|&c| fold(c)).collect();
    let exact: Vec<bool> = q.iter().map(|c| c.is_uppercase()).collect();
    let same = |i: usize, c: char| {
        if exact[i] {
            c == q[i]
        } else {
            fold(c) == qf[i]
        }
    };

    // Not a subsequence at all: the common case, settled without allocating.
    let mut next = 0;
    for c in candidate.chars() {
        if next < m && same(next, c) {
            next += 1;
        }
    }
    if next < m {
        return None;
    }

    let c: Vec<char> = candidate.chars().collect();
    let n = c.len();
    let basename = c
        .iter()
        .rposition(|&ch| ch == '/' || ch == '\\')
        .map_or(0, |i| i + 1);
    let bonus: Vec<i64> = (0..n)
        .map(|j| match j.checked_sub(1).map(|p| c[p]) {
            None => BONUS_BOUNDARY,
            Some('/' | '\\') => BONUS_SLASH,
            Some('_' | '-' | '.' | ' ') => BONUS_BOUNDARY,
            Some(prev) if prev.is_lowercase() && c[j].is_uppercase() => BONUS_CAMEL,
            Some(_) => 0,
        })
        .collect();
    let value = |i: usize, j: usize| {
        let mut v = SCORE_MATCH + if i == 0 { bonus[j] * 2 } else { bonus[j] };
        if j >= basename {
            v += BONUS_BASENAME;
        }
        if j == 0 {
            v += BONUS_PREFIX;
        }
        v
    };

    // best[i][j]: the best score with query char i matched at candidate char j.
    let mut best = vec![NONE; m * n];
    let mut from = vec![usize::MAX; m * n];
    for j in 0..n {
        if same(0, c[j]) {
            best[j] = value(0, j);
        }
    }
    for i in 1..m {
        // The best way to arrive from a match at least one char back (a gap).
        let (mut gap, mut gap_from) = (NONE, usize::MAX);
        for j in i..n {
            if gap > NONE {
                gap -= GAP_EXTEND;
            }
            if j >= 2 {
                let prev = best[(i - 1) * n + j - 2];
                if prev > NONE && prev - GAP_START > gap {
                    (gap, gap_from) = (prev - GAP_START, j - 2);
                }
            }
            if !same(i, c[j]) {
                continue;
            }
            let run = best[(i - 1) * n + j - 1];
            let (arrive, k) = if run > NONE && run + BONUS_CONSECUTIVE >= gap {
                (run + BONUS_CONSECUTIVE, j - 1)
            } else {
                (gap, gap_from)
            };
            if arrive > NONE {
                best[i * n + j] = arrive + value(i, j);
                from[i * n + j] = k;
            }
        }
    }
    let last = (m - 1) * n;
    let (mut j, top) =
        (last..last + n)
            .map(|at| (at - last, best[at]))
            .fold(
                (usize::MAX, NONE),
                |acc, x| if x.1 > acc.1 { x } else { acc },
            );
    if top <= NONE {
        return None;
    }
    let mut indices = vec![0; m];
    for i in (0..m).rev() {
        indices[i] = j;
        if i > 0 {
            j = from[i * n + j];
        }
    }
    Some((top - n as i64 / 8, indices))
}

/// The best `limit` of `paths` for `query`, highest score first; ties go to
/// the shorter path, then the lexicographically smaller. An empty query keeps
/// the first `limit` paths with score 0.
pub fn fuzzy_filter(query: &str, paths: &[String], limit: usize) -> Vec<FuzzyMatch> {
    if limit == 0 {
        return Vec::new();
    }
    if query.chars().all(char::is_whitespace) {
        return paths
            .iter()
            .take(limit)
            .map(|p| FuzzyMatch {
                path: p.clone(),
                score: 0,
                indices: Vec::new(),
            })
            .collect();
    }
    let mut hits: Vec<(i64, usize)> = paths
        .iter()
        .enumerate()
        .filter_map(|(at, p)| fuzzy_match(query, p).map(|(score, _)| (score, at)))
        .collect();
    let order = |a: &(i64, usize), b: &(i64, usize)| {
        let (pa, pb) = (&paths[a.1], &paths[b.1]);
        b.0.cmp(&a.0)
            .then_with(|| pa.chars().count().cmp(&pb.chars().count()))
            .then_with(|| pa.cmp(pb))
    };
    if hits.len() > limit {
        hits.select_nth_unstable_by(limit - 1, order);
        hits.truncate(limit);
    }
    hits.sort_by(order);
    hits.into_iter()
        .map(|(score, at)| FuzzyMatch {
            path: paths[at].clone(),
            score,
            indices: fuzzy_match(query, &paths[at])
                .map(|(_, i)| i)
                .unwrap_or_default(),
        })
        .collect()
}

// ---- content search ----

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GrepHit {
    /// Relative to the search root.
    pub path: String,
    /// 1-based.
    pub line: u64,
    /// 1-based, in characters: the first match on the line.
    pub column: u64,
    /// The line without its line break, cut to 300 characters.
    pub text: String,
    /// Matches as character offsets `[start, end)` in `text`.
    pub ranges: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GrepResult {
    pub hits: Vec<GrepHit>,
    pub truncated: bool,
    /// Files with a hit among `hits`.
    pub files: usize,
}

/// Whether `rg` runs there. Asked once per machine; a machine that could not
/// be reached is asked again next time.
pub fn ripgrep_available(on: Option<&str>) -> bool {
    static KNOWN: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Mutex::default);
    let key = machine(on).unwrap_or_default().to_owned();
    if let Some(&yes) = known.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return yes;
    }
    let req = ExecRequest::new("rg", ["--version"])
        .on(machine(on))
        .timeout(RG_PROBE_TIMEOUT)
        .max_output(4 << 10);
    let yes = match remote::run(&req) {
        Ok(out) => out.code == 0,
        Err(ExecError::Spawn(_)) => false,
        Err(_) => return false,
    };
    known
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, yes);
    yes
}

/// Searches file contents under `root` with ripgrep: a fixed string unless
/// `regex`, smart case unless `case_sensitive`, hidden files when `hidden`,
/// files up to 2 MiB, never `.git`. Stops collecting at `limit` hits.
pub fn grep(
    root: &str,
    on: Option<&str>,
    query: &str,
    regex: bool,
    case_sensitive: bool,
    hidden: bool,
    limit: usize,
) -> Result<GrepResult, String> {
    grep_with(
        "rg",
        &[],
        root,
        on,
        &grep_args(query, regex, case_sensitive, hidden),
        limit,
    )
}

fn grep_args(query: &str, regex: bool, case_sensitive: bool, hidden: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--json",
        "--no-config",
        "--max-filesize",
        "2M",
        "--max-columns",
        "1000",
        "-g",
        "!.git",
    ]
    .map(String::from)
    .into();
    args.push(
        if case_sensitive {
            "--case-sensitive"
        } else {
            "--smart-case"
        }
        .into(),
    );
    if !regex {
        args.push("--fixed-strings".into());
    }
    if hidden {
        args.push("--hidden".into());
    }
    // `-e` takes a pattern that starts with a dash; the explicit path keeps rg
    // from reading the (empty) standard input instead of searching.
    args.extend(["-e".into(), query.into(), "--".into(), ".".into()]);
    args
}

/// `program` with `pre` before `args` (a seam for tests).
fn grep_with(
    program: &str,
    pre: &[&str],
    root: &str,
    on: Option<&str>,
    args: &[String],
    limit: usize,
) -> Result<GrepResult, String> {
    let has_query = args
        .iter()
        .position(|a| a == "-e")
        .and_then(|i| args.get(i + 1))
        .is_some_and(|q| !q.is_empty());
    if !has_query {
        return Ok(GrepResult {
            hits: Vec::new(),
            truncated: false,
            files: 0,
        });
    }
    let argv = pre
        .iter()
        .map(|s| (*s).to_owned())
        .chain(args.iter().cloned());
    let req = ExecRequest::new(program, argv)
        .on(machine(on))
        .cwd(Some(root))
        .timeout(GREP_TIMEOUT)
        .max_output(GREP_MAX_OUTPUT);
    let out = match remote::run(&req) {
        Ok(out) => out,
        Err(ExecError::Spawn(_)) if machine(on).is_none() => {
            return Err("ripgrep (rg) is not installed".into());
        }
        Err(e) => return Err(e.to_string()),
    };
    // At the output cap the last line may be cut: leave it out.
    let capped = out.stdout.len() as u64 >= GREP_MAX_OUTPUT;
    let mut bytes = &out.stdout[..];
    if capped && let Some(nl) = bytes.iter().rposition(|b| *b == b'\n') {
        bytes = &bytes[..=nl];
    }
    let mut result = parse_rg_json(root, &String::from_utf8_lossy(bytes), limit);
    result.truncated |= capped;
    // 0 matches, 1 none; 2 an error, though files it could read still match.
    if !matches!(out.code, 0 | 1) && result.hits.is_empty() {
        return Err(failure(&out, "ripgrep"));
    }
    Ok(result)
}

/// [`grep`] without ripgrep, on this machine: the same walk as [`list_files`],
/// the same rules (fixed string unless `regex`, smart case unless
/// `case_sensitive`, files up to 2 MiB, binary files skipped), the same hits.
/// Stops at `limit` hits or after [`GREP_TIMEOUT`], either marked `truncated`.
pub fn grep_local(
    root: &str,
    query: &str,
    regex: bool,
    case_sensitive: bool,
    hidden: bool,
    limit: usize,
) -> Result<GrepResult, String> {
    let mut result = GrepResult {
        hits: Vec::new(),
        truncated: false,
        files: 0,
    };
    if query.is_empty() {
        return Ok(result);
    }
    let pattern = if regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let insensitive = !case_sensitive && !query.chars().any(char::is_uppercase);
    let matcher = regex::RegexBuilder::new(&pattern)
        .case_insensitive(insensitive)
        .build()
        .map_err(|e| e.to_string())?;
    let base = Path::new(root);
    if !base.is_dir() {
        return Err(format!("cannot read {root}: not a folder"));
    }
    let walk = ignore::WalkBuilder::new(base)
        .hidden(!hidden)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    let deadline = std::time::Instant::now() + GREP_TIMEOUT;
    for entry in walk.flatten() {
        if std::time::Instant::now() >= deadline {
            result.truncated = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file())
            || entry.metadata().map_or(true, |m| m.len() > 2 << 20)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if bytes[..bytes.len().min(8 << 10)].contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(rel) = entry.path().strip_prefix(base) else {
            continue;
        };
        let path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let mut found = false;
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            // A pattern that matches only an empty spot (`^`, `\b`) still
            // makes a hit, as with ripgrep; there is just nothing to mark.
            let mut matches = matcher.find_iter(line).peekable();
            let Some(first) = matches.peek().map(|m| char_offset(line, m.start())) else {
                continue;
            };
            let mut ranges: Vec<(usize, usize)> = matches
                .filter(|m| m.end() > m.start())
                .map(|m| (char_offset(line, m.start()), char_offset(line, m.end())))
                .collect();
            if result.hits.len() >= limit {
                result.truncated = true;
                result.files += usize::from(found);
                return Ok(result);
            }
            found = true;
            let column = first as u64 + 1;
            ranges.retain(|r| r.0 < TEXT_CHARS);
            for r in &mut ranges {
                r.1 = r.1.min(TEXT_CHARS);
            }
            result.hits.push(GrepHit {
                path: path.clone(),
                line: number as u64 + 1,
                column,
                text: line.chars().take(TEXT_CHARS).collect(),
                ranges,
            });
        }
        result.files += usize::from(found);
    }
    Ok(result)
}

/// Reads `rg --json` output: one hit per matching line, at most `limit`.
pub fn parse_rg_json(root: &str, out: &str, limit: usize) -> GrepResult {
    let mut hits = Vec::new();
    let mut truncated = false;
    let mut files = HashSet::new();
    for line in out.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event["type"] != "match" {
            continue;
        }
        let data = &event["data"];
        let (Some(path), Some(text), Some(number)) = (
            data["path"]["text"].as_str(),
            data["lines"]["text"].as_str(),
            data["line_number"].as_u64(),
        ) else {
            continue;
        };
        if hits.len() >= limit {
            truncated = true;
            break;
        }
        let text = text.trim_end_matches(['\n', '\r']);
        // rg counts submatch offsets in bytes of the line.
        let mut ranges: Vec<(usize, usize)> = data["submatches"]
            .as_array()
            .map(|subs| {
                subs.iter()
                    .filter_map(|s| {
                        Some((s["start"].as_u64()? as usize, s["end"].as_u64()? as usize))
                    })
                    .map(|(start, end)| (char_offset(text, start), char_offset(text, end)))
                    .filter(|(start, end)| end > start)
                    .collect()
            })
            .unwrap_or_default();
        let column = ranges.first().map_or(1, |r| r.0 as u64 + 1);
        ranges.retain(|r| r.0 < TEXT_CHARS);
        for r in &mut ranges {
            r.1 = r.1.min(TEXT_CHARS);
        }
        let path = relative(root, path);
        files.insert(path.clone());
        hits.push(GrepHit {
            path,
            line: number,
            column,
            text: text.chars().take(TEXT_CHARS).collect(),
            ranges,
        });
    }
    GrepResult {
        hits,
        truncated,
        files: files.len(),
    }
}

/// Characters of `text` that start before byte `byte`.
fn char_offset(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(at, _)| *at < byte).count()
}

/// rg run in `root` names files `./x/y` (or with `root` in front).
fn relative(root: &str, path: &str) -> String {
    let root_prefix = format!("{}/", root.trim_end_matches(['/', '\\']));
    let path = path
        .strip_prefix(root_prefix.as_str())
        .or_else(|| path.strip_prefix("./"))
        .or_else(|| path.strip_prefix(".\\"))
        .unwrap_or(path);
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn ranked(query: &str, paths: &[&str]) -> Vec<String> {
        let paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
        fuzzy_filter(query, &paths, 100)
            .into_iter()
            .map(|m| m.path)
            .collect()
    }

    fn score(query: &str, path: &str) -> i64 {
        fuzzy_match(query, path).unwrap().0
    }

    #[test]
    fn camel_humps_beat_scattered_letters() {
        assert_eq!(
            ranked("dl", &["docs/old/legacy.txt", "DockLayout.tsx"]),
            ["DockLayout.tsx", "docs/old/legacy.txt"]
        );
        assert!(score("dl", "DockLayout.tsx") > score("dl", "docs/old/legacy.txt"));
    }

    #[test]
    fn word_starts_beat_mid_word_letters() {
        assert_eq!(
            ranked("ft", &["fixture.rs", "file_tree.rs"]),
            ["file_tree.rs", "fixture.rs"]
        );
        assert_eq!(
            ranked("ft", &["fixture.rs", "file-tree.rs"])[0],
            "file-tree.rs"
        );
        assert_eq!(
            ranked("ft", &["fixture.rs", "file.tree.rs"])[0],
            "file.tree.rs"
        );
        assert_eq!(
            ranked("ft", &["fixture.rs", "file tree.rs"])[0],
            "file tree.rs"
        );
        assert_eq!(ranked("fb", &["fabric.rs", "FooBar.rs"])[0], "FooBar.rs");
    }

    #[test]
    fn consecutive_runs_beat_gaps() {
        assert_eq!(
            ranked("abc", &["a_b_c.txt", "abc.txt", "axbxc.txt"])[0],
            "abc.txt"
        );
        assert!(score("abc", "abc.txt") > score("abc", "a_b_c.txt"));
    }

    #[test]
    fn the_basename_beats_a_directory() {
        assert_eq!(
            ranked("main", &["main/src/lib.rs", "src/main.rs"])[0],
            "src/main.rs"
        );
        assert_eq!(
            ranked("foo", &["foo/b/a.rs", "a/b/foo.rs"])[0],
            "a/b/foo.rs"
        );
        // ...and the start of the path beats the middle of a word.
        assert_eq!(ranked("li", &["x/delivery.rs", "lib.rs"])[0], "lib.rs");
    }

    #[test]
    fn shorter_paths_win_ties() {
        assert_eq!(
            ranked("lib", &["crates/a/src/lib.rs", "src/lib.rs"])[0],
            "src/lib.rs"
        );
        let tied = ranked("a", &["b/a", "a/a/a", "c/a"]);
        assert_eq!(tied[0], "b/a");
        assert_eq!(ranked("a", &["c/a", "b/a"]), ["b/a", "c/a"]);
    }

    #[test]
    fn smart_case_makes_uppercase_exact() {
        assert!(fuzzy_match("foo", "Foo.rs").is_some());
        assert!(fuzzy_match("foo", "FOO.rs").is_some());
        assert!(fuzzy_match("Foo", "foo.rs").is_none());
        assert!(fuzzy_match("Foo", "Foo.rs").is_some());
        assert!(fuzzy_match("fOo", "foo.rs").is_none());
        assert!(fuzzy_match("fOo", "fOo.rs").is_some());
        assert_eq!(
            ranked("Dl", &["docs/dl.txt", "DockLayout.tsx"]),
            ["DockLayout.tsx"]
        );
    }

    #[test]
    fn indices_are_character_positions() {
        assert_eq!(fuzzy_match("dl", "DockLayout.tsx").unwrap().1, [0, 4]);
        assert_eq!(fuzzy_match("abc", "abc.txt").unwrap().1, [0, 1, 2]);
        // The best alignment, not the leftmost: "lib" is the basename.
        assert_eq!(fuzzy_match("lib", "a/l/i/lib.rs").unwrap().1, [6, 7, 8]);
        // Characters, not bytes.
        assert_eq!(
            fuzzy_match("\u{6a94}", "\u{76ee}\u{9304}/\u{6a94}\u{6848}.rs")
                .unwrap()
                .1,
            [3]
        );
        assert_eq!(
            fuzzy_match("\u{6a94}\u{6848}r", "\u{76ee}\u{9304}/\u{6a94}\u{6848}.rs")
                .unwrap()
                .1,
            [3, 4, 6]
        );
    }

    #[test]
    fn non_matches_and_edge_queries() {
        assert!(fuzzy_match("xyz", "abc").is_none());
        assert!(fuzzy_match("abcd", "abc").is_none());
        assert!(fuzzy_match("ba", "ab").is_none());
        assert_eq!(fuzzy_match("", "anything"), Some((0, vec![])));
        assert_eq!(fuzzy_match("  ", "anything"), Some((0, vec![])));
        // Spaces in a query are ignored.
        assert_eq!(
            fuzzy_match("my file", "my file.txt").unwrap().1,
            fuzzy_match("myfile", "my file.txt").unwrap().1
        );
        assert!(fuzzy_match("a", "").is_none());
    }

    #[test]
    fn filter_orders_limits_and_keeps_empty_queries_whole() {
        let paths: Vec<String> = [
            "src/lib.rs",
            "src/main.rs",
            "README.md",
            "docs/lib-notes.md",
            "a/b/c/lib.rs",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let hits = fuzzy_filter("lib", &paths, 100);
        let got: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(got, ["src/lib.rs", "a/b/c/lib.rs", "docs/lib-notes.md"]);
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
        assert_eq!(hits[0].indices, [4, 5, 6]);
        let top = fuzzy_filter("lib", &paths, 2);
        assert_eq!(
            top.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            ["src/lib.rs", "a/b/c/lib.rs"]
        );
        assert_eq!(fuzzy_filter("lib", &paths, 0), vec![]);
        let all = fuzzy_filter("", &paths, 3);
        assert_eq!(
            all.iter()
                .map(|h| (h.path.as_str(), h.score, h.indices.len()))
                .collect::<Vec<_>>(),
            [
                ("src/lib.rs", 0, 0),
                ("src/main.rs", 0, 0),
                ("README.md", 0, 0)
            ]
        );
        assert!(fuzzy_filter("zzz", &paths, 10).is_empty());
    }

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join(".gitignore"), "ignored/\n*.log\n").unwrap();
        for dir in ["src", "ignored", ".git", ".config", "empty"] {
            std::fs::create_dir(root.join(dir)).unwrap();
        }
        for f in [
            "README.md",
            "src/main.rs",
            "src/a b.rs",
            "ignored/x.rs",
            "debug.log",
            ".git/HEAD",
            ".config/app.toml",
            ".env",
        ] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        d
    }

    #[test]
    fn list_files_honors_ignore_files_and_skips_git() {
        let d = tree();
        let root = d.path().to_string_lossy().into_owned();
        let list = list_files(&root, None, false, 100).unwrap();
        assert_eq!(list.files, ["README.md", "src/a b.rs", "src/main.rs"]);
        assert!(!list.truncated && !list.unfiltered);
        let all = list_files(&root, None, true, 100).unwrap().files;
        assert_eq!(
            all,
            [
                ".config/app.toml",
                ".env",
                ".gitignore",
                "README.md",
                "src/a b.rs",
                "src/main.rs"
            ]
        );
        let cut = list_files(&root, None, false, 2).unwrap();
        assert_eq!((cut.files.len(), cut.truncated), (2, true));
        assert!(!list_files(&root, None, false, 3).unwrap().truncated);
        assert!(list_files(&format!("{root}/nope"), None, false, 10).is_err());
        assert!(list_files(&format!("{root}/README.md"), None, false, 10).is_err());
        assert_eq!(
            list_files(&format!("{root}/empty"), None, false, 10).unwrap(),
            FileList::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_files_lists_symlinked_files_but_not_folder_links() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("real")).unwrap();
        std::fs::write(d.path().join("real/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink(d.path().join("real/f.txt"), d.path().join("link.txt")).unwrap();
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("dirlink")).unwrap();
        std::os::unix::fs::symlink(d.path().join("gone"), d.path().join("dangling")).unwrap();
        let files = list_files(&d.path().to_string_lossy(), None, false, 10)
            .unwrap()
            .files;
        assert_eq!(files, ["link.txt", "real/f.txt"]);
    }

    #[test]
    fn file_lists_from_git_rg_and_find_are_normalized() {
        let out =
            b"rg\0./src/a.rs\0b.rs\0.git/HEAD\0./.git/config\0.github/ci.yml\0src/.hidden/x\0z.rs\0";
        assert_eq!(
            parse_file_list(out, false, 100),
            FileList {
                files: vec!["b.rs".into(), "src/a.rs".into(), "z.rs".into()],
                ..FileList::default()
            }
        );
        assert_eq!(
            parse_file_list(out, true, 100).files,
            [
                ".github/ci.yml",
                "b.rs",
                "src/.hidden/x",
                "src/a.rs",
                "z.rs"
            ]
        );
        let cut = parse_file_list(out, true, 2);
        assert_eq!(
            (cut.files, cut.truncated),
            (vec![".github/ci.yml".into(), "b.rs".into()], true)
        );
        assert_eq!(parse_file_list(b"", true, 2), FileList::default());
        assert_eq!(
            parse_file_list("rg\0\u{76ee}\u{9304}/\u{6a94}.rs\0".as_bytes(), false, 5).files,
            ["\u{76ee}\u{9304}/\u{6a94}.rs"]
        );
        // git: the tracked files deleted from the work tree follow an empty entry.
        let git = b"git\0a.rs\0gone.rs\0b.rs\0\0gone.rs\0";
        assert_eq!(parse_file_list(git, false, 10).files, ["a.rs", "b.rs"]);
        assert_eq!(parse_file_list(b"git\0\0", false, 10), FileList::default());
        // find applies no ignore file.
        assert!(parse_file_list(b"find\0./a\0", false, 10).unfiltered);
        assert!(!parse_file_list(b"git\0a\0\0", false, 10).unfiltered);
    }

    /// Runs the remote list script with a PATH of the named tools only.
    #[cfg(unix)]
    fn run_list_script(tools: &[(&str, &Path)], root: &Path, hidden: bool) -> (Vec<u8>, i32) {
        let bin = tempfile::tempdir().unwrap();
        for (name, target) in tools {
            std::os::unix::fs::symlink(target, bin.path().join(name)).unwrap();
        }
        let out = Command::new("/bin/sh")
            .args(["-c", LIST_SCRIPT, "sh"])
            .arg(root)
            .arg(if hidden { "1" } else { "0" })
            .env("PATH", bin.path())
            .output()
            .unwrap();
        (out.stdout, out.status.code().unwrap_or(-1))
    }

    #[cfg(unix)]
    fn tool(name: &str) -> Option<std::path::PathBuf> {
        ["/usr/bin", "/bin", "/usr/local/bin", "/opt/homebrew/bin"]
            .iter()
            .map(|d| Path::new(d).join(name))
            .find(|p| p.is_file())
    }

    #[cfg(unix)]
    #[test]
    fn the_list_script_falls_back_from_git_to_rg_to_find() {
        let d = tree();
        // Neither git nor rg: find, with .git left out.
        if let Some(find) = tool("find") {
            let (out, code) = run_list_script(&[("find", &find)], d.path(), true);
            assert_eq!(code, 0);
            let list = parse_file_list(&out, true, 100);
            assert!(list.unfiltered);
            assert_eq!(
                list.files,
                [
                    ".config/app.toml",
                    ".env",
                    ".gitignore",
                    "README.md",
                    "debug.log",
                    "ignored/x.rs",
                    "src/a b.rs",
                    "src/main.rs"
                ]
            );
            let files = parse_file_list(&out, false, 100).files;
            assert!(files.iter().all(|f| !f.starts_with('.')));
        }
        // `echo` standing in for rg shows how it is called.
        if let Some(echo) = tool("echo") {
            let (out, code) = run_list_script(&[("rg", &echo)], d.path(), false);
            assert_eq!(
                (String::from_utf8_lossy(&out).trim_end(), code),
                ("rg\0--files --null -g !.git", 0)
            );
            let (out, _) = run_list_script(&[("rg", &echo)], d.path(), true);
            assert_eq!(
                String::from_utf8_lossy(&out).trim_end(),
                "rg\0--files --null --hidden -g !.git"
            );
        }
        let (out, code) = run_list_script(&[], &d.path().join("nope"), false);
        assert_eq!((out.is_empty(), code), (true, 3));
    }

    #[cfg(unix)]
    #[test]
    fn the_list_script_uses_git_inside_a_work_tree() {
        let Some(git) = tool("git") else {
            eprintln!("git not found: skipping");
            return;
        };
        let d = tree();
        std::fs::remove_dir_all(d.path().join(".git")).unwrap();
        let init = Command::new(&git)
            .args(["init", "-q"])
            .current_dir(d.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(init.success());
        // A tracked file deleted from the work tree is not listed.
        std::fs::write(d.path().join("gone.rs"), "x").unwrap();
        let add = Command::new(&git)
            .args(["add", "--", "gone.rs"])
            .current_dir(d.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(add.success());
        std::fs::remove_file(d.path().join("gone.rs")).unwrap();
        let root = d.path().to_string_lossy().into_owned();
        // Untracked but not ignored files are listed; ignored ones are not.
        let list = list_sh(&root, None, true, 100).unwrap();
        assert_eq!(
            list.files,
            [
                ".config/app.toml",
                ".env",
                ".gitignore",
                "README.md",
                "src/a b.rs",
                "src/main.rs"
            ]
        );
        assert!(!list.truncated && !list.unfiltered);
        let files = list_sh(&root, None, false, 100).unwrap().files;
        assert_eq!(files, ["README.md", "src/a b.rs", "src/main.rs"]);
        let cut = list_sh(&root, None, false, 1).unwrap();
        assert_eq!(
            (cut.files, cut.truncated),
            (vec!["README.md".to_owned()], true)
        );
        // Same answer as the local walk.
        assert_eq!(
            list_local(&root, false, 100).unwrap(),
            list_sh(&root, None, false, 100).unwrap()
        );
        assert!(
            list_sh(&format!("{root}/nope"), None, false, 10)
                .unwrap_err()
                .contains("cannot open")
        );
    }

    const RG_FIXTURE: &str = concat!(
        r#"{"type":"begin","data":{"path":{"text":"./src/main.rs"}}}"#,
        "\n",
        r#"{"type":"match","data":{"path":{"text":"./src/main.rs"},"lines":{"text":"fn main() { println!(\"hi\"); }\n"},"line_number":3,"absolute_offset":20,"submatches":[{"match":{"text":"main"},"start":3,"end":7}]}}"#,
        "\n",
        r#"{"type":"match","data":{"path":{"text":"./src/main.rs"},"lines":{"text":"你好世界 main main\r\n"},"line_number":9,"absolute_offset":80,"submatches":[{"match":{"text":"main"},"start":13,"end":17},{"match":{"text":"main"},"start":18,"end":22}]}}"#,
        "\n",
        r#"{"type":"end","data":{"path":{"text":"./src/main.rs"},"binary_offset":null,"stats":{}}}"#,
        "\n",
        r#"{"type":"begin","data":{"path":{"text":"./b.txt"}}}"#,
        "\n",
        r#"{"type":"match","data":{"path":{"text":"./b.txt"},"lines":{"text":"no submatches here\n"},"line_number":1,"absolute_offset":0,"submatches":[]}}"#,
        "\n",
        r#"{"type":"context","data":{"path":{"text":"./b.txt"},"lines":{"text":"context\n"},"line_number":2,"absolute_offset":19,"submatches":[]}}"#,
        "\n",
        r#"{"type":"match","data":{"path":{"bytes":"Zm9v"},"lines":{"text":"not utf8 path\n"},"line_number":1,"absolute_offset":0,"submatches":[]}}"#,
        "\n",
        r#"{"type":"match","data":{"path":{"text":"./c.txt"},"lines":{"bytes":"Zm9v"},"line_number":1,"absolute_offset":0,"submatches":[]}}"#,
        "\n",
        "not json at all\n",
        r#"{"type":"summary","data":{"elapsed_total":{"secs":0,"nanos":1,"human":"0.000001s"},"stats":{}}}"#,
        "\n",
    );

    #[test]
    fn rg_json_hits_use_character_offsets() {
        let r = parse_rg_json("/work/repo", RG_FIXTURE, 100);
        assert_eq!((r.hits.len(), r.truncated, r.files), (3, false, 2));
        assert_eq!(
            r.hits[0],
            GrepHit {
                path: "src/main.rs".into(),
                line: 3,
                column: 4,
                text: "fn main() { println!(\"hi\"); }".into(),
                ranges: vec![(3, 7)]
            }
        );
        // "你好世界 main main": 4 Chinese characters (12 bytes) then the words.
        let zh = &r.hits[1];
        assert_eq!(
            (zh.path.as_str(), zh.line, zh.column, zh.text.as_str()),
            (
                "src/main.rs",
                9,
                6,
                "\u{4f60}\u{597d}\u{4e16}\u{754c} main main"
            )
        );
        assert_eq!(zh.ranges, [(5, 9), (10, 14)]);
        assert_eq!(zh.text.chars().skip(5).take(4).collect::<String>(), "main");
        // No submatch offsets: column 1, no ranges.
        assert_eq!(
            (
                r.hits[2].path.as_str(),
                r.hits[2].column,
                r.hits[2].ranges.len()
            ),
            ("b.txt", 1, 0)
        );
    }

    #[test]
    fn rg_json_stops_at_the_limit_and_cuts_long_lines() {
        let r = parse_rg_json("/r", RG_FIXTURE, 2);
        assert_eq!((r.hits.len(), r.truncated, r.files), (2, true, 1));
        let r = parse_rg_json("/r", RG_FIXTURE, 3);
        assert_eq!((r.hits.len(), r.truncated), (3, false));
        let r = parse_rg_json("/r", RG_FIXTURE, 0);
        assert_eq!((r.hits.len(), r.truncated, r.files), (0, true, 0));

        let long = format!("{}needle{}", "\u{6f22}".repeat(298), "x".repeat(100));
        let line = format!(
            r#"{{"type":"match","data":{{"path":{{"text":"/r/deep/f.txt"}},"lines":{{"text":"{long}\n"}},"line_number":7,"submatches":[{{"start":894,"end":900}}]}}}}"#
        );
        let r = parse_rg_json("/r", &line, 5);
        let hit = &r.hits[0];
        assert_eq!(hit.path, "deep/f.txt");
        assert_eq!(hit.text.chars().count(), TEXT_CHARS);
        assert_eq!((hit.column, hit.ranges.clone()), (299, vec![(298, 300)]));
        assert_eq!(hit.text.chars().skip(298).collect::<String>(), "ne");
        // A match entirely past the cut keeps its column but has no range.
        let line = line.replace("\"start\":894,\"end\":900", "\"start\":900,\"end\":903");
        let hit = &parse_rg_json("/r", &line, 5).hits[0];
        assert_eq!((hit.column, hit.ranges.len()), (305, 0));
    }

    #[test]
    fn grep_arguments() {
        let a = grep_args("-needle", false, false, false);
        assert!(a.windows(2).any(|w| w == ["-e", "-needle"]));
        assert_eq!(a.last().map(String::as_str), Some("."));
        for want in [
            "--json",
            "--smart-case",
            "--fixed-strings",
            "!.git",
            "2M",
            "1000",
        ] {
            assert!(a.iter().any(|x| x == want), "{want}");
        }
        assert!(!a.iter().any(|x| x == "--hidden" || x == "--case-sensitive"));
        let b = grep_args("a.*b", true, true, true);
        assert!(b.iter().any(|x| x == "--hidden") && b.iter().any(|x| x == "--case-sensitive"));
        assert!(
            !b.iter()
                .any(|x| x == "--fixed-strings" || x == "--smart-case")
        );
    }

    /// `grep_with` against a stand-in for rg: a shell script that prints a
    /// fixture and exits with a chosen code.
    #[cfg(unix)]
    fn with_fake_rg(
        stdout: &str,
        stderr: &str,
        code: i32,
        limit: usize,
    ) -> (Result<GrepResult, String>, Vec<String>) {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("rg.sh");
        std::fs::write(d.path().join("out"), stdout).unwrap();
        std::fs::write(d.path().join("err"), stderr).unwrap();
        std::fs::write(&script, format!("printf '%s\\n' \"$@\" > '{0}/args'\ncat '{0}/out'\ncat '{0}/err' >&2\nexit {code}\n", d.path().display())).unwrap();
        let args = grep_args("main", false, false, false);
        let result = grep_with(
            "sh",
            &[&script.to_string_lossy()],
            &d.path().to_string_lossy(),
            None,
            &args,
            limit,
        );
        let seen = std::fs::read_to_string(d.path().join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        (result, seen)
    }

    #[cfg(unix)]
    #[test]
    fn grep_reads_rg_exit_codes() {
        let (hits, seen) = with_fake_rg(RG_FIXTURE, "", 0, 100);
        assert_eq!(hits.unwrap().hits.len(), 3);
        assert_eq!(seen, grep_args("main", false, false, false));
        // 1 is "no matches".
        let (none, _) = with_fake_rg("", "", 1, 100);
        assert_eq!(
            none.unwrap(),
            GrepResult {
                hits: vec![],
                truncated: false,
                files: 0
            }
        );
        // 2 with nothing found is an error with rg's words...
        let (bad, _) = with_fake_rg("", "rg: regex parse error: unclosed group\n", 2, 100);
        assert_eq!(bad.unwrap_err(), "rg: regex parse error: unclosed group");
        let (bad, _) = with_fake_rg("", "", 2, 100);
        assert!(bad.unwrap_err().contains("exit 2"));
        // ...but with hits (an unreadable file elsewhere) the hits stand.
        let (partial, _) = with_fake_rg(RG_FIXTURE, "rg: ./secret: Permission denied\n", 2, 100);
        assert_eq!(partial.unwrap().hits.len(), 3);
        let (cut, _) = with_fake_rg(RG_FIXTURE, "", 0, 1);
        let cut = cut.unwrap();
        assert_eq!((cut.hits.len(), cut.truncated), (1, true));
        // An empty query is no search at all.
        assert_eq!(
            grep("/", None, "", false, false, false, 10)
                .unwrap()
                .hits
                .len(),
            0
        );
    }

    #[test]
    fn grep_without_rg_says_so() {
        let r = grep_with(
            "yuzora-no-such-rg",
            &[],
            "/",
            None,
            &grep_args("x", false, false, false),
            10,
        );
        assert_eq!(r.unwrap_err(), "ripgrep (rg) is not installed");
    }

    #[test]
    fn ripgrep_is_asked_once() {
        let first = ripgrep_available(None);
        assert_eq!(ripgrep_available(None), first);
    }

    #[test]
    fn a_real_ripgrep_search() {
        if !ripgrep_available(None) {
            eprintln!("rg not found: skipping");
            return;
        }
        let d = tree();
        std::fs::write(
            d.path().join("src/main.rs"),
            "fn main() {}\n\u{4f60}\u{597d} Main\nlast\n",
        )
        .unwrap();
        std::fs::write(d.path().join("ignored/x.rs"), "main in an ignored folder\n").unwrap();
        let root = d.path().to_string_lossy().into_owned();
        let r = grep(&root, None, "main", false, false, false, 100).unwrap();
        let found: Vec<(&str, u64, u64)> = r
            .hits
            .iter()
            .map(|h| (h.path.as_str(), h.line, h.column))
            .collect();
        assert_eq!(found, [("src/main.rs", 1, 4), ("src/main.rs", 2, 4)]);
        assert_eq!(r.hits[1].ranges, [(3, 7)]);
        assert_eq!(r.files, 1);
        // Smart case turns off with case_sensitive.
        assert_eq!(
            grep(&root, None, "main", false, true, false, 100)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            grep(&root, None, "Main", false, false, false, 100)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            grep(&root, None, "m.in", true, false, false, 100)
                .unwrap()
                .hits
                .len(),
            2
        );
        assert_eq!(
            grep(&root, None, "m.in", false, false, false, 100)
                .unwrap()
                .hits
                .len(),
            0
        );
        assert!(grep(&root, None, "(", true, false, false, 100).is_err());
        assert_eq!(
            grep(&root, None, "nothing like this", false, false, false, 100)
                .unwrap()
                .hits
                .len(),
            0
        );
        let cut = grep(&root, None, "main", false, false, false, 1).unwrap();
        assert_eq!((cut.hits.len(), cut.truncated), (1, true));
    }

    #[test]
    fn the_built_in_search_answers_like_ripgrep() {
        let d = tree();
        std::fs::write(
            d.path().join("src/main.rs"),
            "fn main() {}\r\n\u{4f60}\u{597d} Main\nlast\n",
        )
        .unwrap();
        std::fs::write(d.path().join("ignored/x.rs"), "main in an ignored folder\n").unwrap();
        std::fs::write(d.path().join("blob.bin"), b"main\0\x01").unwrap();
        let root = d.path().to_string_lossy().into_owned();
        let r = grep_local(&root, "main", false, false, false, 100).unwrap();
        let found: Vec<(&str, u64, u64)> = r
            .hits
            .iter()
            .map(|h| (h.path.as_str(), h.line, h.column))
            .collect();
        assert_eq!(found, [("src/main.rs", 1, 4), ("src/main.rs", 2, 4)]);
        assert_eq!(r.hits[0].text, "fn main() {}");
        assert_eq!(r.hits[1].ranges, [(3, 7)]);
        assert_eq!(r.files, 1);
        assert_eq!(
            grep_local(&root, "main", false, true, false, 100)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            grep_local(&root, "Main", false, false, false, 100)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            grep_local(&root, "m.in", true, false, false, 100)
                .unwrap()
                .hits
                .len(),
            2
        );
        assert_eq!(
            grep_local(&root, "m.in", false, false, false, 100)
                .unwrap()
                .hits
                .len(),
            0
        );
        assert!(grep_local(&root, "(", true, false, false, 100).is_err());
        assert_eq!(
            grep_local(&root, "", false, false, false, 100)
                .unwrap()
                .hits
                .len(),
            0
        );
        let cut = grep_local(&root, "main", false, false, false, 1).unwrap();
        assert_eq!((cut.hits.len(), cut.truncated, cut.files), (1, true, 1));
        // A pattern that only matches an empty spot finds lines, with nothing to mark.
        let starts = grep_local(&root, "^", true, false, false, 100).unwrap();
        let main: Vec<_> = starts
            .hits
            .iter()
            .filter(|h| h.path == "src/main.rs")
            .collect();
        assert_eq!(main.len(), 3);
        assert!(main.iter().all(|h| h.column == 1 && h.ranges.is_empty()));
        let words = grep_local(&root, r"\blast", true, false, false, 100).unwrap();
        assert_eq!(
            words
                .hits
                .iter()
                .map(|h| (h.line, h.column))
                .collect::<Vec<_>>(),
            [(3, 1)]
        );
    }
}
