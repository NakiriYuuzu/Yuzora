//! herdr-core agent templates: `<config>/herdr-core/templates/<name>.md`.
//!
//! ```markdown
//! ---
//! description: Read-only code review. Give it file paths and scope.
//! command: pi
//! ---
//! You are a focused review agent. …
//! ```
//!
//! The header is herdr-core's own small line format, not YAML, and this module
//! reads it the way `hc` does (checked against hc 0.1.13):
//!
//! - the first line is exactly `---`; the header ends at the next line that is
//!   `---` once trailing blanks are trimmed;
//! - every other header line is `key: value` (split at the first colon, both
//!   sides trimmed) or blank; keys are case-sensitive; a repeated key: the last wins;
//! - `command` is required; `paste_gap_ms` is a whole number of milliseconds;
//! - unknown keys are ignored by hc and kept here as written.
//!
//! The filename without `.md` is the name (a `name:` field is ignored).

use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The fields the editor shows, in the order a saved file lists them.
pub const FIELDS: &[&str] = &["command", "description", "ready", "submit", "stop", "paste_gap_ms"];

/// Longest file read (hc's templates are a few lines; anything bigger is a mistake).
const MAX_FILE: u64 = 1 << 20;

/// One template file, read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    /// Every header line, `(key, value)`, in file order (repeats kept).
    pub header: Vec<(String, String)>,
    /// Everything after the closing `---` line, as written.
    pub body: String,
}

impl Template {
    /// A field's value: the last one written, `""` when absent.
    pub fn field(&self, key: &str) -> &str {
        self.header.iter().rev().find(|(k, _)| k == key).map_or("", |(_, v)| v.as_str())
    }

    /// Header lines this editor has no field for, last write of each key, in order.
    pub fn others(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (k, v) in &self.header {
            if FIELDS.contains(&k.as_str()) {
                continue;
            }
            match out.iter_mut().find(|(o, _)| o == k) {
                Some(slot) => slot.1 = v.clone(),
                None => out.push((k.clone(), v.clone())),
            }
        }
        out
    }

    /// The known fields as a JSON object (`""` for the absent ones).
    pub fn fields_json(&self) -> Value {
        Value::Object(FIELDS.iter().map(|f| ((*f).to_owned(), json!(self.field(f)))).collect())
    }
}

/// Why a file isn't a template (hc's own reasons).
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    /// The first line isn't `---` (a blank line or CRLF before it counts).
    NoFrontmatter,
    /// No closing `---` line.
    NeverCloses,
    /// A header line without a colon.
    NotKeyValue(String),
}

impl ParseError {
    pub fn code(&self) -> &'static str {
        match self {
            ParseError::NoFrontmatter => "no_frontmatter",
            ParseError::NeverCloses => "never_closes",
            ParseError::NotKeyValue(_) => "not_key_value",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            ParseError::NotKeyValue(line) => Some(line),
            _ => None,
        }
    }
}

pub fn parse(text: &str) -> Result<Template, ParseError> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first.strip_suffix('\n').unwrap_or(first) != "---" {
        return Err(ParseError::NoFrontmatter);
    }
    // The closing line first, then the lines before it (hc reports a file
    // that never closes before a bad line in it).
    let mut consumed = first.len();
    let mut inside = Vec::new();
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        let bare = line.strip_suffix('\n').unwrap_or(line);
        if bare.trim_end() == "---" {
            closed = true;
            break;
        }
        inside.push(bare);
    }
    if !closed {
        return Err(ParseError::NeverCloses);
    }
    let mut header = Vec::new();
    for bare in inside.into_iter().filter(|l| !l.trim().is_empty()) {
        let Some((k, v)) = bare.split_once(':') else {
            return Err(ParseError::NotKeyValue(bare.trim().to_owned()));
        };
        header.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    Ok(Template { header, body: text[consumed..].to_owned() })
}

/// The file text for `fields` (known keys, `""` = leave out), the `others`
/// kept from the file, and `body`.
pub fn render(fields: &Value, others: &[(String, String)], body: &str) -> String {
    let mut out = String::from("---\n");
    for f in FIELDS {
        let v = fields.get(*f).and_then(Value::as_str).unwrap_or_default().trim();
        if !v.is_empty() {
            out.push_str(&format!("{f}: {v}\n"));
        }
    }
    for (k, v) in others {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push_str("---\n");
    out.push_str(body);
    if !body.is_empty() && !body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// One thing wrong with a template before it's saved.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// `name`, a field, or `body`.
    pub field: String,
    /// What's wrong, for the UI's strings (`issue.<code>`).
    pub code: &'static str,
    /// An error blocks saving; a warning doesn't.
    pub error: bool,
}

impl Issue {
    fn new(field: &str, code: &'static str, error: bool) -> Self {
        Self { field: field.to_owned(), code, error }
    }

    pub fn to_json(&self) -> Value {
        json!({ "field": self.field, "code": self.code, "level": if self.error { "error" } else { "warning" } })
    }
}

/// A name `hc run <name>` finds as one file in the folder. hc takes any file
/// name (`my review`, `審查`, `-x`); this refuses only what isn't one plain
/// file there: a path separator, a control character, a leading `.` (hidden,
/// like the temporary files a save writes), more than 200 bytes.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && !name.starts_with('.') && !name.chars().any(|c| matches!(c, '/' | '\\') || c.is_control())
}

/// Key names `hc keys` takes (`enter`, `esc`, `ctrl+c`, `alt+x`, `f5`, `a`…),
/// space-separated. Unknown ones only warn: hc may know more than this list.
fn known_keys(value: &str) -> bool {
    const NAMES: &[&str] = &[
        "enter", "return", "esc", "escape", "tab", "space", "backspace", "delete", "del", "insert", "up", "down", "left",
        "right", "home", "end", "pageup", "pagedown",
    ];
    value.split_whitespace().all(|token| {
        let t = token.to_ascii_lowercase();
        let mut parts: Vec<&str> = t.split('+').collect();
        let key = parts.pop().unwrap_or_default();
        let mods_ok = parts.iter().all(|m| matches!(*m, "ctrl" | "alt" | "shift" | "meta" | "cmd" | "super"));
        let fkey = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
        mods_ok && (key.chars().count() == 1 || fkey || NAMES.contains(&key))
    })
}

/// What stops a template from saving (errors) or may not do what was meant (warnings).
pub fn check(name: &str, fields: &Value, body: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if name.trim().is_empty() {
        issues.push(Issue::new("name", "name_empty", true));
    } else if !valid_name(name) {
        issues.push(Issue::new("name", "name_invalid", true));
    }
    let get = |f: &str| fields.get(f).and_then(Value::as_str).unwrap_or_default();
    for f in FIELDS {
        let v = get(f);
        if v.contains(['\n', '\r']) {
            issues.push(Issue::new(f, "multiline", true));
        }
        let t = v.trim();
        if t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\''))) {
            issues.push(Issue::new(f, "quoted", false));
        }
    }
    let command = get("command").trim();
    if command.is_empty() {
        issues.push(Issue::new("command", "command_empty", true));
    }
    // hc --skill: Codex must run with --no-daemon, or its commands run outside hc.
    if command.split_whitespace().next().is_some_and(|p| p.rsplit('/').next() == Some("codex")) && !command.contains("--no-daemon") {
        issues.push(Issue::new("command", "codex_daemon", false));
    }
    let gap = get("paste_gap_ms").trim();
    if !gap.is_empty() && gap.parse::<u64>().is_err() {
        issues.push(Issue::new("paste_gap_ms", "paste_gap_number", true));
    }
    let ready = get("ready").trim();
    if !ready.is_empty() && ready != "quiet" {
        issues.push(Issue::new("ready", "ready_unknown", false));
    }
    for f in ["submit", "stop"] {
        let v = get(f).trim();
        if !v.is_empty() && !known_keys(v) {
            issues.push(Issue::new(f, "keys_unknown", false));
        }
    }
    // The body is one message: a header-looking first line is fine, but say so.
    if body.trim().is_empty() {
        issues.push(Issue::new("body", "body_empty", false));
    }
    issues
}

/// herdr-core's templates folder here: `$XDG_CONFIG_HOME/herdr-core/templates`,
/// else `%APPDATA%\herdr-core\templates` on Windows, else `~/.config/herdr-core/templates`.
pub fn dir() -> PathBuf {
    dir_from(|k| std::env::var_os(k), yuzora_core::paths::home_dir(), cfg!(windows))
}

pub fn dir_from(env: impl Fn(&str) -> Option<OsString>, home: PathBuf, windows: bool) -> PathBuf {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let config = var("XDG_CONFIG_HOME").or_else(|| if windows { var("APPDATA") } else { None }).unwrap_or_else(|| home.join(".config"));
    config.join("herdr-core").join("templates")
}

/// One `<name>.md` and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub text: String,
    pub parsed: Result<Template, ParseError>,
}

impl Entry {
    pub fn new(name: String, path: String, text: String) -> Self {
        let parsed = parse(&text);
        Self { name, path, text, parsed }
    }

    /// hc's own verdict, as `hc templates` would give it.
    pub fn error(&self) -> Option<(&'static str, Option<String>)> {
        match &self.parsed {
            Err(e) => Some((e.code(), e.detail().map(str::to_owned))),
            Ok(t) if t.field("command").is_empty() => Some(("no_command", None)),
            Ok(t) if !t.field("paste_gap_ms").is_empty() && t.field("paste_gap_ms").parse::<u64>().is_err() => {
                Some(("paste_gap_number", None))
            }
            Ok(_) => None,
        }
    }

    /// Runnable: hc would start it.
    pub fn ok(&self) -> bool {
        self.error().is_none()
    }

    pub fn to_json(&self) -> Value {
        let (fields, others, body) = match &self.parsed {
            Ok(t) => (t.fields_json(), t.others(), t.body.clone()),
            Err(_) => (Template::default().fields_json(), Vec::new(), self.text.clone()),
        };
        let issues: Vec<Value> = match &self.parsed {
            Ok(t) => check(&self.name, &t.fields_json(), &t.body).iter().map(Issue::to_json).collect(),
            Err(_) => Vec::new(),
        };
        json!({
            "name": self.name,
            "path": self.path,
            "text": self.text,
            "fields": fields,
            "others": others.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "body": body,
            "error": self.error().map(|(code, detail)| json!({ "code": code, "detail": detail })),
            "issues": issues,
        })
    }
}

/// Every `*.md` file directly in `dir` (as hc lists them: lower-case `.md`,
/// no subfolders), by name. A missing folder has none.
pub fn list(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".md")).map(str::to_owned) else { continue };
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_file() || name.is_empty() {
            continue;
        }
        let text = if meta.len() > MAX_FILE {
            String::new()
        } else {
            match std::fs::read(&path) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => continue,
            }
        };
        out.push(Entry::new(name, path.to_string_lossy().into_owned(), text));
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Why a save didn't happen.
#[derive(Debug)]
pub enum SaveError {
    /// The name isn't one hc can run.
    Name,
    /// Another template already has that name.
    Exists,
    /// The file changed (or went) since it was read.
    Changed,
    Io(std::io::Error),
    /// Renamed: the new file is written, the old one couldn't go to the trash
    /// (both are there now).
    OldKept(std::io::Error),
}

impl SaveError {
    pub fn code(&self) -> &'static str {
        match self {
            SaveError::Name => "name_invalid",
            SaveError::Exists => "exists",
            SaveError::Changed => "changed",
            SaveError::Io(_) => "io",
            SaveError::OldKept(_) => "old_kept",
        }
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "io: {e}"),
            SaveError::OldKept(e) => write!(f, "old_kept: {e}"),
            other => f.write_str(other.code()),
        }
    }
}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// Writes `<dir>/<name>.md`.
///
/// - `expected` `None`: a new template; refused when the name is taken.
/// - `expected` `Some(text)`: an edit of the file read as `text` (of
///   `previous` when renamed); refused when it changed since.
/// - `previous`: the old name when renamed; that file goes to the trash
///   once the new one is written.
pub fn save(dir: &Path, name: &str, text: &str, expected: Option<&str>, previous: Option<&str>) -> Result<(), SaveError> {
    if !valid_name(name) || previous.is_some_and(|p| !valid_name(p)) {
        return Err(SaveError::Name);
    }
    let target = dir.join(format!("{name}.md"));
    let source = previous.filter(|p| *p != name).map(|p| dir.join(format!("{p}.md")));
    match expected {
        None => {
            if target.exists() {
                return Err(SaveError::Exists);
            }
        }
        Some(expected) => {
            let from = source.as_ref().unwrap_or(&target);
            let now = std::fs::read(from).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|_| SaveError::Changed)?;
            if now != expected {
                return Err(SaveError::Changed);
            }
            if source.is_some() && target.exists() {
                return Err(SaveError::Exists);
            }
        }
    }
    std::fs::create_dir_all(dir)?;
    // An edit of a linked template (dotfiles) writes the file it points at,
    // keeping the link.
    let linked = source.is_none() && std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink());
    let target = if linked { std::fs::canonicalize(&target)? } else { target };
    let tmp = target.with_file_name(format!(".{name}.md.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Some(old) = source {
        crate::trash::trash(&old).map_err(SaveError::OldKept)?;
    }
    Ok(())
}

/// Moves `<dir>/<name>.md` to the trash.
pub fn trash(dir: &Path, name: &str) -> std::io::Result<()> {
    if !valid_name(name) {
        return Err(std::io::Error::other("not a template name"));
    }
    crate::trash::trash(&dir.join(format!("{name}.md")))
}

// ---- another machine, through `hc exec --on` and its sh ----------------------

/// The folder (first line), then each `.md` file as `\x1e<file>\x1f<text>`.
pub const REMOTE_LIST: &str = r#"d="${XDG_CONFIG_HOME:-$HOME/.config}/herdr-core/templates"
printf '%s\n' "$d"
[ -d "$d" ] || exit 0
for f in "$d"/*.md; do
  [ -f "$f" ] || continue
  printf '\036%s\037' "${f##*/}"
  cat "$f"
done
"#;

/// A move to the trash on that machine: gio, a `trash` command (macOS 15+
/// ships one), else the freedesktop trash folder by hand.
const REMOTE_TRASH_FN: &str = r#"trash_file() {
  if command -v gio >/dev/null 2>&1 && gio trash "$1" 2>/dev/null; then return 0; fi
  if command -v trash >/dev/null 2>&1 && trash "$1" 2>/dev/null; then return 0; fi
  t="${XDG_DATA_HOME:-$HOME/.local/share}/Trash"
  mkdir -p "$t/files" "$t/info" || return 1
  b=${1##*/}; n=$b; i=1
  while [ -e "$t/files/$n" ] || [ -e "$t/info/$n.trashinfo" ]; do i=$((i+1)); n="${b%.md}.$i.md"; done
  printf '[Trash Info]\nPath=%s\nDeletionDate=%s\n' "$1" "$(date +%Y-%m-%dT%H:%M:%S)" > "$t/info/$n.trashinfo" && mv "$1" "$t/files/$n"
}
"#;

/// `sh -c REMOTE_SAVE sh <name> <text> <new|old> <expected> <previous>`.
/// Exits 4 when the name is taken, 5 when the file changed, 6 on a write error,
/// 7 when renamed but the old file couldn't go to the trash.
pub fn remote_save_script() -> String {
    format!(
        r#"{REMOTE_TRASH_FN}d="${{XDG_CONFIG_HOME:-$HOME/.config}}/herdr-core/templates"
name=$1; text=$2; mode=$3; expected=$4; previous=$5
mkdir -p "$d" || exit 6
f="$d/$name.md"; src=$f
if [ -n "$previous" ] && [ "$previous" != "$name" ]; then src="$d/$previous.md"; fi
if [ "$mode" = new ]; then
  [ -e "$f" ] && exit 4
else
  [ -f "$src" ] || exit 5
  cur=$(cat "$src"; printf x); cur=${{cur%x}}
  [ "$cur" = "$expected" ] || exit 5
  if [ "$src" != "$f" ] && [ -e "$f" ]; then exit 4; fi
fi
if [ "$src" = "$f" ] && [ -L "$f" ]; then
  printf '%s' "$text" > "$f" || exit 6
else
  tmp="$d/.$name.md.tmp.$$"
  printf '%s' "$text" > "$tmp" && mv -f "$tmp" "$f" || {{ rm -f "$tmp"; exit 6; }}
fi
if [ "$src" != "$f" ]; then trash_file "$src" || exit 7; fi
"#
    )
}

/// `sh -c REMOTE_TRASH sh <name>`; exits 3 when there's no such template.
pub fn remote_trash_script() -> String {
    format!(
        r#"{REMOTE_TRASH_FN}f="${{XDG_CONFIG_HOME:-$HOME/.config}}/herdr-core/templates/$1.md"
[ -f "$f" ] || exit 3
trash_file "$f" || exit 7
"#
    )
}

/// Reads [`REMOTE_LIST`]'s output: the folder and the templates in it.
pub fn parse_remote_list(out: &str) -> (String, Vec<Entry>) {
    let (dir, rest) = out.split_once('\n').unwrap_or((out, ""));
    let dir = dir.trim_end_matches('/').to_owned();
    let mut entries: Vec<Entry> = rest
        .split('\u{1e}')
        .filter(|p| !p.is_empty())
        .filter_map(|part| {
            let (file, text) = part.split_once('\u{1f}')?;
            let name = file.strip_suffix(".md")?.to_owned();
            Some(Entry::new(name, format!("{dir}/{file}"), text.to_owned()))
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    (dir, entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yuzora-templates-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_the_header_as_hc_does() {
        let t = parse("---\nname: ignored\n  command  :  pi --model x  \n\ndescription: Review: read-only\ncommand: claude\n---\nYou review.\n\nOnly that.\n").unwrap();
        assert_eq!(t.field("command"), "claude", "the last one wins");
        assert_eq!(t.field("description"), "Review: read-only", "split at the first colon");
        assert_eq!(t.field("ready"), "");
        assert_eq!(t.others(), [("name".to_owned(), "ignored".to_owned())]);
        assert_eq!(t.body, "You review.\n\nOnly that.\n");
        // The closing line may have trailing blanks; the opening one may not.
        assert!(parse("---\ncommand: x\n---   \nb").is_ok());
        assert_eq!(parse("---  \ncommand: x\n---\n"), Err(ParseError::NoFrontmatter));
        assert_eq!(parse("\n---\ncommand: x\n---\n"), Err(ParseError::NoFrontmatter));
        assert_eq!(parse("---\r\ncommand: x\r\n---\r\n"), Err(ParseError::NoFrontmatter), "CRLF is not ---");
        assert_eq!(parse("---\ncommand: x\n"), Err(ParseError::NeverCloses));
        assert_eq!(parse("---\n# a comment\n---\n"), Err(ParseError::NotKeyValue("# a comment".into())));
        assert_eq!(parse("---\ncommand: x\n  ---\n"), Err(ParseError::NeverCloses), "an indented --- doesn't close, and hc says so first");
        assert_eq!(parse(""), Err(ParseError::NoFrontmatter));
    }

    #[test]
    fn errors_match_hc_templates() {
        let e = |text: &str| Entry::new("x".into(), "x.md".into(), text.into()).error().map(|(c, _)| c);
        assert_eq!(e("---\ncommand: pi\n---\n"), None);
        assert_eq!(e("---\nCommand: pi\n---\n"), Some("no_command"), "keys are case-sensitive");
        assert_eq!(e("---\ncommand:\n---\n"), Some("no_command"));
        assert_eq!(e("---\ncommand: pi\npaste_gap_ms: -5\n---\n"), Some("paste_gap_number"));
        assert_eq!(e("---\ncommand: pi\npaste_gap_ms: 1.5\n---\n"), Some("paste_gap_number"));
        assert_eq!(e("---\ncommand: pi\npaste_gap_ms: 99999999999999999999\n---\n"), Some("paste_gap_number"));
        assert_eq!(e("---\ncommand: pi\npaste_gap_ms: 100\nready: bogus\n---\n"), None, "hc doesn't refuse ready");
        assert_eq!(e("no header"), Some("no_frontmatter"));
    }

    #[test]
    fn writes_what_it_reads_back() {
        let fields = json!({ "command": "pi", "description": "Review", "ready": "", "submit": "enter", "stop": "", "paste_gap_ms": "100" });
        let text = render(&fields, &[("model".into(), "x".into())], "Be careful.");
        assert_eq!(text, "---\ncommand: pi\ndescription: Review\nsubmit: enter\npaste_gap_ms: 100\nmodel: x\n---\nBe careful.\n");
        let t = parse(&text).unwrap();
        assert_eq!(t.fields_json(), fields);
        assert_eq!(t.others(), [("model".to_owned(), "x".to_owned())]);
        assert_eq!(render(&fields, &t.others(), &t.body), text);
    }

    #[test]
    fn checks_before_saving() {
        let codes = |name: &str, fields: Value, body: &str| check(name, &fields, body).into_iter().map(|i| (i.field, i.code, i.error)).collect::<Vec<_>>();
        assert_eq!(codes("rev", json!({ "command": "pi" }), "b"), []);
        assert_eq!(codes("", json!({ "command": "pi" }), "b"), [("name".into(), "name_empty", true)]);
        assert_eq!(codes("a/b", json!({ "command": "pi" }), "b"), [("name".into(), "name_invalid", true)]);
        assert_eq!(codes(".x", json!({ "command": "pi" }), "b")[0].1, "name_invalid");
        for name in ["my review", "審查", "-x"] {
            assert!(codes(name, json!({ "command": "pi" }), "b").is_empty(), "hc runs {name}");
        }
        assert_eq!(codes("ok", json!({}), "b"), [("command".into(), "command_empty", true)]);
        assert_eq!(codes("ok", json!({ "command": "pi\nrm" }), "b"), [("command".into(), "multiline", true)]);
        assert_eq!(codes("ok", json!({ "command": "\"pi\"" }), "b"), [("command".into(), "quoted", false)]);
        assert_eq!(codes("ok", json!({ "command": "pi", "paste_gap_ms": "fast" }), "b"), [("paste_gap_ms".into(), "paste_gap_number", true)]);
        assert_eq!(codes("ok", json!({ "command": "pi", "ready": "now" }), "b"), [("ready".into(), "ready_unknown", false)]);
        assert_eq!(codes("ok", json!({ "command": "pi", "submit": "ctrl+j enter", "stop": "esc ctrl+c" }), "b"), []);
        assert_eq!(codes("ok", json!({ "command": "pi", "submit": "nonsense-key" }), "b"), [("submit".into(), "keys_unknown", false)]);
        assert_eq!(codes("ok", json!({ "command": "pi" }), " \n"), [("body".into(), "body_empty", false)]);
        assert_eq!(codes("ok", json!({ "command": "codex --model x" }), "b"), [("command".into(), "codex_daemon", false)]);
        assert_eq!(codes("ok", json!({ "command": "/opt/bin/codex --no-daemon" }), "b"), []);
    }

    #[test]
    fn the_folder_follows_xdg_then_appdata() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v));
        assert_eq!(dir_from(env(&[]), "/h".into(), false), PathBuf::from("/h/.config/herdr-core/templates"));
        assert_eq!(dir_from(env(&[("XDG_CONFIG_HOME", "/x")]), "/h".into(), false), PathBuf::from("/x/herdr-core/templates"));
        assert_eq!(dir_from(env(&[("APPDATA", "/a")]), "/h".into(), false), PathBuf::from("/h/.config/herdr-core/templates"));
        assert_eq!(dir_from(env(&[("APPDATA", "/a")]), "/h".into(), true), PathBuf::from("/a/herdr-core/templates"));
        assert_eq!(dir_from(env(&[("APPDATA", "/a"), ("XDG_CONFIG_HOME", "/x")]), "/h".into(), true), PathBuf::from("/x/herdr-core/templates"));
    }

    #[test]
    fn lists_md_files_only() {
        let d = tmp("list");
        std::fs::write(d.join("b.md"), "---\ncommand: pi\n---\nhi\n").unwrap();
        std::fs::write(d.join("a.md"), "nope").unwrap();
        std::fs::write(d.join("c.MD"), "---\ncommand: pi\n---\n").unwrap();
        std::fs::write(d.join("notes.txt"), "").unwrap();
        std::fs::create_dir_all(d.join("sub.md")).unwrap();
        let l = list(&d).unwrap();
        assert_eq!(l.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(!l[0].ok());
        assert!(l[1].ok());
        assert_eq!(l[1].to_json()["fields"]["command"], "pi");
        assert_eq!(l[0].to_json()["error"]["code"], "no_frontmatter");
        assert!(list(&d.join("missing")).unwrap().is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn saves_new_edits_and_renames_without_clobbering() {
        let d = tmp("save");
        let one = "---\ncommand: pi\n---\none\n";
        save(&d, "rev", one, None, None).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("rev.md")).unwrap(), one);
        assert!(matches!(save(&d, "rev", one, None, None), Err(SaveError::Exists)));
        assert!(matches!(save(&d, "rev", "x", Some("stale"), None), Err(SaveError::Changed)));
        assert!(matches!(save(&d, "../evil", "x", None, None), Err(SaveError::Name)));
        let two = "---\ncommand: claude\n---\ntwo\n";
        save(&d, "rev", two, Some(one), None).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("rev.md")).unwrap(), two);
        std::fs::write(d.join("taken.md"), "x").unwrap();
        assert!(matches!(save(&d, "taken", two, Some(two), Some("rev")), Err(SaveError::Exists)));
        assert!(matches!(save(&d, "gone", two, Some(two), Some("missing")), Err(SaveError::Changed)));
        assert!(d.join("rev.md").exists(), "a refused rename leaves the file");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A template linked from dotfiles stays a link; its file gets the edit.
    #[cfg(unix)]
    #[test]
    fn an_edit_writes_through_a_link() {
        let d = tmp("link");
        let dots = d.join("dotfiles");
        let dir = d.join("templates");
        std::fs::create_dir_all(&dots).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let one = "---\ncommand: pi\n---\none\n";
        std::fs::write(dots.join("rev.md"), one).unwrap();
        std::os::unix::fs::symlink(dots.join("rev.md"), dir.join("rev.md")).unwrap();
        save(&dir, "rev", "two", Some(one), None).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("rev.md")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(dots.join("rev.md")).unwrap(), "two");
        assert_eq!(std::fs::read_dir(&dots).unwrap().count(), 1, "no temporary file left beside it");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn reads_another_machines_listing() {
        let out = "/home/p/.config/herdr-core/templates\n\u{1e}rev.md\u{1f}---\ncommand: pi\n---\nhi\n\u{1e}bad.md\u{1f}oops";
        let (dir, l) = parse_remote_list(out);
        assert_eq!(dir, "/home/p/.config/herdr-core/templates");
        assert_eq!(l.iter().map(|e| (e.name.as_str(), e.ok())).collect::<Vec<_>>(), [("bad", false), ("rev", true)]);
        assert_eq!(l[1].path, "/home/p/.config/herdr-core/templates/rev.md");
        assert_eq!(parse_remote_list("/d\n").1, []);
    }

    /// The remote scripts on this machine's sh, with a throwaway config home
    /// and trash: no gio or trash command on PATH, so nothing reaches the
    /// person's own trash.
    #[cfg(unix)]
    #[test]
    fn remote_scripts_work_in_sh() {
        let d = tmp("sh");
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in ["cat", "mkdir", "mv", "rm", "date"] {
            let found = ["/bin", "/usr/bin"].iter().map(|p| Path::new(p).join(tool)).find(|p| p.exists()).unwrap();
            std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
        }
        let sh = |script: &str, args: &[&str]| {
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .arg("sh")
                .args(args)
                .env("XDG_CONFIG_HOME", d.join("config"))
                .env("XDG_DATA_HOME", d.join("data"))
                .env("PATH", &bin)
                .output()
                .unwrap()
        };
        let save = remote_save_script();
        let text = "---\ncommand: pi\n---\nends with blank lines\n\n";
        assert_eq!(sh(&save, &["rev", text, "new", "", ""]).status.code(), Some(0));
        assert_eq!(sh(&save, &["rev", text, "new", "", ""]).status.code(), Some(4), "taken");
        assert_eq!(sh(&save, &["rev", "x", "old", "stale", ""]).status.code(), Some(5), "changed");
        assert_eq!(sh(&save, &["rev", text, "old", text, ""]).status.code(), Some(0), "trailing newlines compare exactly");
        let out = sh(REMOTE_LIST, &[]);
        let (dir, l) = parse_remote_list(&String::from_utf8_lossy(&out.stdout));
        assert!(dir.ends_with("config/herdr-core/templates"));
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].text, text);
        // Rename: the new file is written, the old one leaves the folder.
        assert_eq!(sh(&save, &["review", text, "old", text, "rev"]).status.code(), Some(0));
        let names = |out: std::process::Output| parse_remote_list(&String::from_utf8_lossy(&out.stdout)).1.into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(sh(REMOTE_LIST, &[])), ["review"]);
        assert_eq!(sh(&remote_trash_script(), &["review"]).status.code(), Some(0));
        assert_eq!(sh(&remote_trash_script(), &["review"]).status.code(), Some(3));
        assert!(names(sh(REMOTE_LIST, &[])).is_empty());
        let trashed = d.join("data/Trash");
        assert!(trashed.join("files/rev.md").exists() && trashed.join("info/rev.md.trashinfo").exists());
        assert_eq!(std::fs::read_to_string(trashed.join("files/review.md")).unwrap(), text);
        // A linked template: the edit reaches the file, the link stays; the
        // trash takes the link, never the file.
        let dots = d.join("dotfiles/lab.md");
        std::fs::create_dir_all(dots.parent().unwrap()).unwrap();
        std::fs::write(&dots, text).unwrap();
        let link = d.join("config/herdr-core/templates/lab.md");
        std::os::unix::fs::symlink(&dots, &link).unwrap();
        assert_eq!(sh(&save, &["lab", "edited", "old", text, ""]).status.code(), Some(0));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&dots).unwrap(), "edited");
        assert_eq!(sh(&remote_trash_script(), &["lab"]).status.code(), Some(0));
        assert!(std::fs::symlink_metadata(trashed.join("files/lab.md")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&dots).unwrap(), "edited");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
