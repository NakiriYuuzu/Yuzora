//! Search for the Files lane: the file list for quick open, fuzzy matching of
//! paths, and content search through ripgrep. Blocking. Here or on a
//! herdr-core machine (`on: Some(machine)`, POSIX paths).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use yuzora_core::git_trust;
use yuzora_core::remote::{ExecError, ExecOutput, ExecRequest};

use crate::fsops::{self, failure, machine, run_sh};

const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_MAX_OUTPUT: u64 = 32 << 20;
const GREP_TIMEOUT: Duration = Duration::from_secs(20);
const GREP_MAX_OUTPUT: u64 = 8 << 20;
const RG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// A hit's line is cut to this many characters.
const TEXT_CHARS: usize = 300;
/// The built-in search skips files larger than this.
const GREP_FILE_MAX: u64 = 2 << 20;

/// The files under `$1` (relative): git's list in a work tree git may read
/// (`$3` is 1, see [`crate::git::work_tree_open`]), else ripgrep's, else
/// find's. `$2` is 1 to include hidden files. Prints the tool's name, then
/// NUL-terminated paths; git's are followed by an empty entry and the
/// tracked files deleted from the work tree. find knows no ignore files.
const LIST_SCRIPT: &str = r#"cd -- "$1" 2>/dev/null || { echo "cannot open $1" >&2; exit 3; }
g() { git --no-optional-locks -c core.fsmonitor=false "$@"; }
if [ "$3" = 1 ] && command -v git >/dev/null 2>&1 && [ "$(g rev-parse --is-inside-work-tree 2>/dev/null)" = true ]; then
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
    /// A folder could not be read: files may be missing.
    pub skipped: bool,
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
    let mut skipped = false;
    for entry in walk {
        let Ok(entry) = entry else {
            skipped = true;
            continue;
        };
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
                skipped,
                unfiltered: false,
            });
        }
        let Some(name) = utf8_path(rel) else {
            skipped = true;
            continue;
        };
        files.push(name);
    }
    Ok(FileList {
        files,
        skipped,
        ..FileList::default()
    })
}

fn list_sh(root: &str, on: Option<&str>, hidden: bool, limit: usize) -> Result<FileList, String> {
    list_sh_with(&git_trust::trusted_file(), root, on, hidden, limit)
}

/// [`list_sh`] with the trusted folders listed in `trusted`. A work tree
/// that waits for trust is listed by ripgrep or find: git's listing reads
/// the work tree and the index (a partial clone's sparse index may fetch).
fn list_sh_with(
    trusted: &Path,
    root: &str,
    on: Option<&str>,
    hidden: bool,
    limit: usize,
) -> Result<FileList, String> {
    let git = crate::git::work_tree_open(trusted, root, on);
    let out = run_sh(
        on,
        LIST_SCRIPT,
        &[
            root,
            if hidden { "1" } else { "0" },
            if git { "1" } else { "0" },
        ],
        LIST_TIMEOUT,
        LIST_MAX_OUTPUT,
    )?;
    read_listing(&out, hidden, limit)
}

/// What [`LIST_SCRIPT`] printed and how it ended.
fn read_listing(out: &ExecOutput, hidden: bool, limit: usize) -> Result<FileList, String> {
    // find and rg exit non-zero for an unreadable corner or an empty tree and
    // still print what they found.
    let tag = out.stdout.iter().position(|b| *b == 0);
    let listed = tag.is_some_and(|tag| tag + 1 < out.stdout.len());
    if !listed && !matches!(out.code, 0 | 1) {
        return Err(failure(out, "listing the files"));
    }
    // rg's 1 is "no files"; any other tool's, or rg's 2, an error on the way.
    let tool = &out.stdout[..tag.unwrap_or(0)];
    let failed = if tool == b"rg" {
        out.code > 1
    } else {
        out.code != 0
    };
    let capped = out.stdout.len() as u64 >= LIST_MAX_OUTPUT;
    let mut bytes = &out.stdout[..];
    if capped && let Some(nul) = bytes.iter().rposition(|b| *b == 0) {
        bytes = &bytes[..=nul];
    }
    let mut list = parse_file_list(bytes, hidden, limit);
    list.truncated |= capped;
    // Even with nothing listed: find's 1 may be an unreadable folder first.
    list.skipped |= failed;
    Ok(list)
}

/// What [`LIST_SCRIPT`] prints: the tool, then paths with `./` dropped,
/// git's deleted files, `.git` and (unless `hidden`) dot-files left out;
/// sorted, cut to `limit`. A path that isn't UTF-8 can't be opened by name:
/// left out, and the list said to be [`FileList::skipped`].
fn parse_file_list(out: &[u8], hidden: bool, limit: usize) -> FileList {
    let mut entries = out.split(|b| *b == 0);
    let tool = entries.next().unwrap_or_default();
    let listed: Vec<&[u8]> = entries.by_ref().take_while(|p| !p.is_empty()).collect();
    let gone: HashSet<&[u8]> = entries.filter(|p| !p.is_empty()).collect();
    let mut skipped = false;
    let mut files: Vec<String> = listed
        .into_iter()
        .filter(|p| !gone.contains(p))
        .map(|p| (String::from_utf8_lossy(p), std::str::from_utf8(p).is_ok()))
        .map(|(p, utf8)| (p.strip_prefix("./").unwrap_or(&p).to_owned(), utf8))
        .filter(|(p, _)| {
            p.split('/')
                .all(|part| part != ".git" && (hidden || !part.starts_with('.')))
        })
        .filter_map(|(p, utf8)| {
            skipped |= !utf8;
            utf8.then_some(p)
        })
        .collect();
    files.sort();
    let truncated = files.len() > limit;
    files.truncate(limit);
    FileList {
        files,
        truncated,
        unfiltered: tool == b"find",
        skipped,
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

/// [`fuzzy_filter`], and whether more paths matched than the `limit` shown.
pub fn fuzzy_top(query: &str, paths: &[String], limit: usize) -> (Vec<FuzzyMatch>, bool) {
    let mut hits = fuzzy_filter(query, paths, limit.saturating_add(1));
    let more = hits.len() > limit;
    hits.truncate(limit);
    (hits, more)
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
    /// A file or folder could not be read: hits may be missing.
    pub skipped: bool,
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
    let yes = match fsops::run(req) {
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
            skipped: false,
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
    let out = match fsops::run(req) {
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
    result.skipped |= !matches!(out.code, 0 | 1);
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
        skipped: false,
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
    for entry in walk {
        let Ok(entry) = entry else {
            result.skipped = true;
            continue;
        };
        if std::time::Instant::now() >= deadline {
            result.truncated = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file())
            || entry.metadata().map_or(true, |m| m.len() > GREP_FILE_MAX)
        {
            continue;
        }
        // It may have grown since: never more than the limit is read.
        let bytes = match read_at_most(entry.path(), GREP_FILE_MAX) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(_) => {
                result.skipped = true;
                continue;
            }
        };
        if bytes[..bytes.len().min(8 << 10)].contains(&0) {
            continue;
        }
        // Not UTF-8 throughout: searched as shown, each bad byte a U+FFFD
        // (as rg's lines are).
        let text = String::from_utf8_lossy(&bytes);
        let Ok(rel) = entry.path().strip_prefix(base) else {
            continue;
        };
        let path = utf8_path(rel);
        let mut found = false;
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            // A pattern that matches only an empty spot (`^`, `\b`) still
            // makes a hit, as with ripgrep; there is just nothing to mark.
            let matches = matcher.find_iter(line).map(|m| (m.start(), m.end()));
            let Some((column, ranges)) = marks(|b| char_offset(line, b), matches) else {
                continue;
            };
            let Some(path) = &path else {
                result.skipped = true;
                break;
            };
            if result.hits.len() >= limit {
                result.truncated = true;
                result.files += usize::from(found);
                return Ok(result);
            }
            found = true;
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

/// Reads `rg --json` output: one hit per matching line, at most `limit`. A
/// line that isn't UTF-8 (base64 `bytes`) is shown with replacement
/// characters; a path that isn't can't be opened by name: left out, and the
/// result said to be [`GrepResult::skipped`].
pub fn parse_rg_json(root: &str, out: &str, limit: usize) -> GrepResult {
    let mut hits = Vec::new();
    let mut truncated = false;
    let mut skipped = false;
    let mut files = HashSet::new();
    for line in out.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event["type"] != "match" {
            continue;
        }
        let data = &event["data"];
        let Some(number) = data["line_number"].as_u64() else {
            continue;
        };
        let lines = &data["lines"];
        let raw: Option<Cow<[u8]>> = match lines["text"].as_str() {
            Some(text) => Some(Cow::Borrowed(text.as_bytes())),
            None => lines["bytes"]
                .as_str()
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
                .map(Cow::Owned),
        };
        let (Some(path), Some(raw)) = (data["path"]["text"].as_str(), raw) else {
            skipped = true;
            continue;
        };
        if hits.len() >= limit {
            truncated = true;
            break;
        }
        let end = raw.len()
            - raw
                .iter()
                .rev()
                .take_while(|b| matches!(b, b'\n' | b'\r'))
                .count();
        let raw = &raw[..end];
        let text = String::from_utf8_lossy(raw);
        // rg counts submatch offsets in bytes of the line.
        let subs = data["submatches"].as_array().into_iter().flatten();
        let subs =
            subs.filter_map(|s| Some((s["start"].as_u64()? as usize, s["end"].as_u64()? as usize)));
        let (column, ranges) = marks(|b| chars_before(raw, b), subs).unwrap_or((1, Vec::new()));
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
        skipped,
    }
}

/// [`char_offset`] in a line's bytes, counted as [`String::from_utf8_lossy`]
/// shows them.
fn chars_before(raw: &[u8], byte: usize) -> usize {
    String::from_utf8_lossy(&raw[..byte.min(raw.len())])
        .chars()
        .count()
}

/// A file's bytes if it holds at most `cap`; reads one byte past it at most.
fn read_at_most(path: &Path, cap: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    crate::fsops::open_regular(path)?
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= cap).then_some(bytes))
}

/// Characters of `text` that start before byte `byte`.
fn char_offset(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(at, _)| *at < byte).count()
}

/// A hit's column (1-based) and the character ranges to mark, from its
/// matches in order (byte offsets; `chars_at` turns one into characters).
/// The first, even empty (`$`), is where the hit goes; empty ones mark
/// nothing. Only matches that start in the [`TEXT_CHARS`] shown are looked
/// at, so a line with millions of them costs no more than a short one.
/// `None` without any match.
fn marks(
    chars_at: impl Fn(usize) -> usize,
    mut matches: impl Iterator<Item = (usize, usize)>,
) -> Option<(u64, Vec<(usize, usize)>)> {
    let first = matches.next()?;
    let column = chars_at(first.0) as u64 + 1;
    let mut ranges = Vec::new();
    for (start, end) in std::iter::once(first).chain(matches) {
        let from = chars_at(start);
        if from >= TEXT_CHARS {
            break;
        }
        let to = chars_at(end).min(TEXT_CHARS);
        if to > from {
            ranges.push((from, to));
        }
    }
    Some((column, ranges))
}

/// `rel` with `/` between its parts; None when one isn't UTF-8: such a path
/// can't be opened by name (a lossy one could name another file).
fn utf8_path(rel: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    parts.map(|parts| parts.join("/"))
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
        // More matches than shown is said, and only then.
        let (shown, more) = fuzzy_top("lib", &paths, 2);
        assert_eq!((shown, more), (top, true));
        assert!(!fuzzy_top("lib", &paths, hits.len()).1);
        assert_eq!(fuzzy_top("lib", &paths, hits.len()).0, hits);
        assert!(fuzzy_top("", &paths, paths.len() - 1).1);
        assert!(!fuzzy_top("", &paths, paths.len()).1);
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

    #[cfg(unix)]
    #[test]
    fn a_shut_folder_here_is_said_to_be_skipped() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_string_lossy().into_owned();
        std::fs::write(d.path().join("a.txt"), "main\n").unwrap();
        let shut = d.path().join("shut");
        std::fs::create_dir(&shut).unwrap();
        std::fs::write(shut.join("b.txt"), "main\n").unwrap();
        let open = || {
            let list = list_files(&root, None, false, 100).unwrap();
            let grep = grep_local(&root, "main", false, false, false, 100).unwrap();
            (
                list.files.len(),
                list.skipped,
                grep.hits.len(),
                grep.skipped,
            )
        };
        assert_eq!(open(), (2, false, 2, false));
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let seen = std::fs::read_dir(&shut).is_ok();
        let shut_out = open();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !seen {
            assert_eq!(shut_out, (1, true, 1, true));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_built_in_search_reads_no_more_than_its_limit() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        std::fs::write(&f, "0123456789").unwrap();
        assert_eq!(
            read_at_most(&f, 10).unwrap().as_deref(),
            Some(&b"0123456789"[..])
        );
        assert_eq!(read_at_most(&f, 9).unwrap(), None);
        // A device that never ends is not read at all.
        assert!(read_at_most(Path::new("/dev/zero"), 16).is_err());
    }

    #[test]
    fn a_listing_cut_short_by_an_error_says_so() {
        let ended = |stdout: &[u8], code: i32| {
            let out = ExecOutput {
                code,
                stdout: stdout.to_vec(),
                stderr: b"Permission denied".to_vec(),
            };
            read_listing(&out, false, 100)
        };
        let whole = ended(b"rg\0a.rs\0", 0).unwrap();
        assert_eq!((whole.files.len(), whole.skipped), (1, false));
        // rg's 2, or any failure of find or git after some files: partial.
        for (out, code) in [
            (&b"rg\0a.rs\0"[..], 2),
            (b"find\0./a.rs\0", 1),
            (b"git\0a.rs\0\0", 128),
        ] {
            let list = ended(out, code).unwrap();
            assert_eq!((list.files, list.skipped), (vec!["a.rs".to_owned()], true));
        }
        // rg's 1 with nothing listed is an empty tree; other failures with nothing are errors.
        assert_eq!(ended(b"rg\0", 1).unwrap(), FileList::default());
        assert_eq!(ended(b"rg\0", 2), Err("Permission denied".to_owned()));
        // find's 1 before it found anything: no files, but not all of them.
        let shut = ended(b"find\0", 1).unwrap();
        assert_eq!(
            (shut.files.len(), shut.unfiltered, shut.skipped),
            (0, true, true)
        );
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
        // A name that isn't UTF-8 (another machine's): left out, and said so,
        // through a run that otherwise went well.
        let odd = parse_file_list(b"find\0./a.rs\0./b\xff.rs\0", false, 10);
        assert_eq!((odd.files, odd.skipped), (vec!["a.rs".to_owned()], true));
        let hidden = parse_file_list(b"find\0./.b\xff\0", false, 10);
        assert_eq!((hidden.files.len(), hidden.skipped), (0, false));
        let out = ExecOutput {
            code: 0,
            stdout: b"find\0./a.rs\0./b\xff.rs\0".to_vec(),
            stderr: Vec::new(),
        };
        assert!(read_listing(&out, false, 10).unwrap().skipped);
    }

    /// [`list_sh`] here with no folder trusted (a plain repo needs none).
    fn listed(root: &str, hidden: bool, limit: usize) -> Result<FileList, String> {
        list_sh_with(
            Path::new("/nonexistent/trusted.json"),
            root,
            None,
            hidden,
            limit,
        )
    }

    /// Runs the remote list script with a PATH of the named tools only;
    /// `git` says git may read the work tree.
    #[cfg(unix)]
    fn run_list_script(
        tools: &[(&str, &Path)],
        root: &Path,
        hidden: bool,
        git: bool,
    ) -> (Vec<u8>, i32) {
        let bin = tempfile::tempdir().unwrap();
        for (name, target) in tools {
            std::os::unix::fs::symlink(target, bin.path().join(name)).unwrap();
        }
        let out = Command::new("/bin/sh")
            .args(["-c", LIST_SCRIPT, "sh"])
            .arg(root)
            .arg(if hidden { "1" } else { "0" })
            .arg(if git { "1" } else { "0" })
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
            let (out, code) = run_list_script(&[("find", &find)], d.path(), true, true);
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
            let (out, code) = run_list_script(&[("rg", &echo)], d.path(), false, true);
            assert_eq!(
                (String::from_utf8_lossy(&out).trim_end(), code),
                ("rg\0--files --null -g !.git", 0)
            );
            let (out, _) = run_list_script(&[("rg", &echo)], d.path(), true, true);
            assert_eq!(
                String::from_utf8_lossy(&out).trim_end(),
                "rg\0--files --null --hidden -g !.git"
            );
        }
        let (out, code) = run_list_script(&[], &d.path().join("nope"), false, true);
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
        let list = listed(&root, true, 100).unwrap();
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
        let files = listed(&root, false, 100).unwrap().files;
        assert_eq!(files, ["README.md", "src/a b.rs", "src/main.rs"]);
        let cut = listed(&root, false, 1).unwrap();
        assert_eq!(
            (cut.files, cut.truncated),
            (vec!["README.md".to_owned()], true)
        );
        // Same answer as the local walk.
        assert_eq!(
            list_local(&root, false, 100).unwrap(),
            listed(&root, false, 100).unwrap()
        );
        assert!(
            listed(&format!("{root}/nope"), false, 10)
                .unwrap_err()
                .contains("cannot open")
        );
    }

    /// A sparse index whose folder's tree the clone lacks, from a promisor
    /// remote whose URL names a remote helper: `ls-files` starts the helper
    /// to fetch it. The listing never does.
    #[cfg(unix)]
    #[test]
    fn the_listing_fetches_nothing_a_partial_clone_lacks() {
        use std::os::unix::fs::PermissionsExt;
        let Some(git) = tool("git") else {
            eprintln!("git not found: skipping");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join("in")).unwrap();
        std::fs::create_dir_all(root.join("out/deep")).unwrap();
        for (f, body) in [("a.txt", "1"), ("in/b.txt", "2"), ("out/deep/c.txt", "3")] {
            std::fs::write(root.join(f), body).unwrap();
        }
        let run = |args: &[&str]| {
            let out = Command::new(&git)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
        };
        run(&["init", "-q"]);
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "one"]);
        run(&["sparse-checkout", "set", "--cone", "--sparse-index", "in"]);
        run(&["config", "remote.origin.url", "zz://payload"]);
        run(&["config", "remote.origin.promisor", "true"]);
        // `out`'s tree is what git would fetch to expand the index.
        let tree = run(&["rev-parse", "HEAD:out"]);
        std::fs::remove_file(root.join(format!(".git/objects/{}/{}", &tree[..2], &tree[2..])))
            .unwrap();
        // The helper git finds on PATH for `zz://` leaves a mark.
        let (bin, marker) = (tmp.path().join("bin"), tmp.path().join("ran"));
        std::fs::create_dir(&bin).unwrap();
        let helper = bin.join("git-remote-zz");
        std::fs::write(
            &helper,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let top = root.to_string_lossy().into_owned();
        // The listing as `list_sh` runs it, git allowed.
        let listing = || {
            fsops::sh_request(
                None,
                LIST_SCRIPT,
                &[&top, "0", "1"],
                LIST_TIMEOUT,
                LIST_MAX_OUTPUT,
            )
            .env("PATH", &path)
        };

        let out = fsops::run(listing()).unwrap();
        assert!(
            !marker.exists(),
            "the listing started the promisor's remote helper"
        );
        assert!(out.stdout.starts_with(b"git\0"), "git did not list it");
        assert!(
            read_listing(&out, false, 100)
                .unwrap()
                .files
                .iter()
                .any(|f| f == "a.txt")
        );
        // Run as it was before, it does: the marker is a live oracle.
        let _ = yuzora_core::remote::run(&listing());
        assert!(
            marker.exists(),
            "ls-files never started the helper, so this proves nothing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_untrusted_work_tree_is_listed_without_git() {
        use std::os::unix::fs::PermissionsExt;
        let Some(git) = tool("git") else {
            eprintln!("git not found: skipping");
            return;
        };
        let run = |args: &[&str], dir: &Path| {
            let out = Command::new(&git)
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
        };
        let d = tree();
        std::fs::remove_dir_all(d.path().join(".git")).unwrap();
        let root = d.path().to_string_lossy().into_owned();
        let state = tempfile::tempdir().unwrap();
        let trusted = state.path().join("trusted.json");
        let open = |cwd: &str| crate::git::work_tree_open(&trusted, cwd, None);
        assert!(!open(&root), "not a work tree");
        run(&["init", "-q"], d.path());
        assert!(open(&root), "a plain repo needs no trust");
        // An ignored file that is tracked: only git's list has it.
        run(&["add", "-f", "--", "debug.log"], d.path());
        let by_git =
            |list: &FileList| list.files.iter().any(|f| f == "debug.log") && !list.unfiltered;
        assert!(by_git(
            &list_sh_with(&trusted, &root, None, false, 100).unwrap()
        ));

        // The repo's own config names a program git runs on the work tree.
        run(&["config", "filter.probe.clean", "cat"], d.path());
        assert!(!open(&root) && !open(&format!("{root}/src")));
        let list = list_sh_with(&trusted, &root, None, false, 100).unwrap();
        assert!(!by_git(&list), "git listed an untrusted work tree");
        assert!(list.files.iter().any(|f| f == "src/main.rs"));
        // Trusted: git lists it again.
        let top = run(&["rev-parse", "--show-toplevel"], d.path());
        git_trust::trust_at(&trusted, &git_trust::folder_id(None, &top)).unwrap();
        assert!(open(&root));
        assert!(by_git(
            &list_sh_with(&trusted, &root, None, false, 100).unwrap()
        ));

        // Told no, the script never calls git: one that leaves a mark.
        let bin = tempfile::tempdir().unwrap();
        let marker = bin.path().join("ran");
        let fake = bin.path().join("fake-git");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\n: > '{}'\necho true\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (out, _) = run_list_script(&[("git", &fake)], d.path(), false, false);
        assert!(!marker.exists(), "the list script ran git");
        assert!(out.starts_with(b"find\0"));
        let (out, _) = run_list_script(&[("git", &fake)], d.path(), false, true);
        assert!(
            marker.exists(),
            "the fake git never ran, so this proves nothing"
        );
        assert!(out.starts_with(b"git\0"));
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
        // The match whose path is bytes is left out (and said to be).
        assert_eq!(
            (r.hits.len(), r.truncated, r.files, r.skipped),
            (4, false, 3, true)
        );
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
        // A line given as bytes is decoded.
        assert_eq!(
            (r.hits[3].path.as_str(), r.hits[3].text.as_str()),
            ("c.txt", "foo")
        );
    }

    #[test]
    fn an_empty_rg_match_still_places_the_hit() {
        // `$` on "abc": a match at the end, nothing to mark.
        let out = r#"{"type":"match","data":{"path":{"text":"./a.txt"},"lines":{"text":"abc\n"},"line_number":2,"absolute_offset":0,"submatches":[{"match":{"text":""},"start":3,"end":3}]}}"#;
        let r = parse_rg_json("/r", out, 10);
        assert_eq!((r.hits[0].column, r.hits[0].ranges.len()), (4, 0));
    }

    #[test]
    fn rg_json_stops_at_the_limit_and_cuts_long_lines() {
        let r = parse_rg_json("/r", RG_FIXTURE, 2);
        assert_eq!((r.hits.len(), r.truncated, r.files), (2, true, 1));
        let r = parse_rg_json("/r", RG_FIXTURE, 4);
        assert_eq!((r.hits.len(), r.truncated), (4, false));
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
    fn rg_lines_and_paths_that_are_not_utf8() {
        let line = r#"{"type":"match","data":{"path":{"text":"/r/a.txt"},"lines":{"bytes":"LINE"},"line_number":3,"submatches":[{"match":{"text":"needle"},"start":5,"end":11}]}}"#
            .replace("LINE", "Y2Fm6SBuZWVkbGUK");
        let r = parse_rg_json("/r", &line, 5);
        assert!(!r.skipped);
        let hit = &r.hits[0];
        assert_eq!(hit.text, "caf\u{fffd} needle");
        assert_eq!(
            (hit.line, hit.column, hit.ranges.clone()),
            (3, 6, vec![(5, 11)])
        );
        // A path rg can only give as bytes: not opened, said to be missing.
        let path = line.replace(r#"{"text":"/r/a.txt"}"#, r#"{"bytes":"L3IvYmFk/y50eHQ="}"#);
        let r = parse_rg_json("/r", &path, 5);
        assert_eq!((r.hits.len(), r.files, r.skipped), (0, 0, true));
    }

    #[test]
    fn only_matches_in_the_shown_text_are_looked_at() {
        let pulled = std::cell::Cell::new(0);
        let matches = (0..1_000_000).map(|i| {
            pulled.set(pulled.get() + 1);
            (i, i + 1)
        });
        let (column, ranges) = marks(|b| b, matches).unwrap();
        assert_eq!((column, ranges.len()), (1, TEXT_CHARS));
        assert_eq!(pulled.get(), TEXT_CHARS + 1);
        // A first match past the shown text: its column, nothing marked.
        assert_eq!(marks(|b| b, [(400, 403)].into_iter()), Some((401, vec![])));
        assert_eq!(marks(|b| b, std::iter::empty()), None);
        // A long line of matches, searched without rg: one hit, quickly.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a".repeat(1 << 20)).unwrap();
        let r = grep_local(dir.path().to_str().unwrap(), "a", false, false, false, 10);
        assert_eq!(
            r.map(|r| (r.hits.len(), r.hits[0].ranges.len())),
            Ok((1, TEXT_CHARS))
        );
    }

    #[test]
    fn the_builtin_search_reads_a_file_that_is_not_all_utf8() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"caf\xe9 needle\nneedle two\n").unwrap();
        let root = dir.path().to_str().unwrap();
        let r = grep_local(root, "needle", false, false, false, 10).unwrap();
        let hits: Vec<_> = r
            .hits
            .iter()
            .map(|h| (h.line, h.column, h.text.as_str(), h.ranges.clone()))
            .collect();
        assert_eq!(
            hits,
            [
                (1, 6, "caf\u{fffd} needle", vec![(5, 11)]),
                (2, 1, "needle two", vec![(0, 6)])
            ]
        );
        assert_eq!((r.files, r.skipped), (1, false));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_left_out_and_said_to_be() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;
        let odd = OsStr::from_bytes(b"b\xff.txt");
        assert_eq!(utf8_path(Path::new("a/b.txt")), Some("a/b.txt".into()));
        assert_eq!(utf8_path(&Path::new("a").join(odd)), None);
        // Searched without rg, on a file system that takes such a name (not APFS).
        let dir = tempfile::tempdir().unwrap();
        if std::fs::write(dir.path().join(odd), "needle\n").is_err() {
            eprintln!("skipped: this file system takes only UTF-8 names");
            return;
        }
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let root = dir.path().to_str().unwrap();
        let r = grep_local(root, "needle", false, false, false, 10).unwrap();
        let hits: Vec<&str> = r.hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!((hits, r.files, r.skipped), (vec!["a.txt"], 1, true));
        let l = list_files(root, None, false, 10).unwrap();
        assert_eq!((l.files, l.skipped), (vec!["a.txt".to_owned()], true));
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
        assert_eq!(hits.unwrap().hits.len(), 4);
        assert_eq!(seen, grep_args("main", false, false, false));
        // 1 is "no matches".
        let (none, _) = with_fake_rg("", "", 1, 100);
        assert_eq!(
            none.unwrap(),
            GrepResult {
                hits: vec![],
                truncated: false,
                files: 0,
                skipped: false
            }
        );
        // 2 with nothing found is an error with rg's words...
        let (bad, _) = with_fake_rg("", "rg: regex parse error: unclosed group\n", 2, 100);
        assert_eq!(bad.unwrap_err(), "rg: regex parse error: unclosed group");
        let (bad, _) = with_fake_rg("", "", 2, 100);
        assert!(bad.unwrap_err().contains("exit 2"));
        // ...but with hits (an unreadable file elsewhere) the hits stand.
        let (partial, _) = with_fake_rg(RG_FIXTURE, "rg: ./secret: Permission denied\n", 2, 100);
        let partial = partial.unwrap();
        assert_eq!((partial.hits.len(), partial.skipped), (4, true));
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
