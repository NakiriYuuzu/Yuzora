//! Fail-closed static preview allowlist.
//!
//! Discovery walks only statically declared HTML / CSS / JS references. Secret,
//! dotfile, VCS, symlink, and non-UTF-8 paths are never added. Oversized graphs
//! abort session creation instead of serving a truncated tree.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use scraper::{Html, Selector};

pub const MAX_DISCOVERY_DEPTH: usize = 8;
pub const MAX_DISCOVERY_FILES: usize = 64;
pub const MAX_DISCOVERY_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_SERVE_BYTES: u64 = 8 * 1024 * 1024;

const DENIED_BASENAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.development",
    ".env.production",
    ".env.test",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".pgpass",
    ".htpasswd",
    ".htaccess",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ecdsa_sk",
    "id_ed25519_sk",
    "authorized_keys",
    "known_hosts",
    "credentials",
    "credentials.json",
    "secrets.json",
    "secrets.yaml",
    "secrets.yml",
    "service-account.json",
    "git-credentials",
];

const DENIED_EXTENSIONS: &[&str] = &[
    "env", "pem", "key", "p12", "pfx", "crt", "cer", "der", "jks", "keystore", "kdbx", "ovpn",
];

const VCS_COMPONENTS: &[&str] = &[".git", ".svn", ".hg", ".bzr", "cvs"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    NotAccessible,
    NotAFile,
    Denied,
    GraphTooLarge,
    InvalidPath,
}

impl PolicyError {
    pub fn as_code(self) -> &'static str {
        match self {
            Self::NotAccessible => "preview-not-accessible",
            Self::NotAFile => "preview-not-a-file",
            Self::Denied => "preview-not-allowed",
            Self::GraphTooLarge => "preview-graph-too-large",
            Self::InvalidPath => "preview-invalid-path",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Html,
    Css,
    Js,
    Other,
}

#[derive(Debug, Clone)]
pub struct Allowlist {
    pub selected: PathBuf,
    pub url_root: PathBuf,
    pub files: HashSet<PathBuf>,
}

pub fn build_allowlist(selected_html: &Path) -> Result<Allowlist, PolicyError> {
    let selected = inspect_selected_html(selected_html)?;
    let site_root = selected
        .parent()
        .ok_or(PolicyError::InvalidPath)?
        .to_path_buf();
    let mut discovery = Discovery {
        files: HashSet::new(),
        bytes_read: 0,
        site_root: site_root.clone(),
    };
    discovery.visit(selected.clone(), AssetKind::Html, 0)?;
    if discovery.files.is_empty() {
        return Err(PolicyError::NotAFile);
    }
    Ok(Allowlist {
        selected,
        url_root: site_root,
        files: discovery.files,
    })
}

pub fn is_denied_path(path: &Path) -> bool {
    if !path_is_operational_utf8(path) {
        return true;
    }
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let Some(name) = name.to_str() else {
                    return true;
                };
                if name.contains('\0') || component_is_denied(name) {
                    return true;
                }
            }
            Component::ParentDir => return true,
            _ => {}
        }
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

pub fn classify_kind(path: &Path) -> AssetKind {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => AssetKind::Html,
        Some("css") => AssetKind::Css,
        Some("js") | Some("mjs") | Some("cjs") => AssetKind::Js,
        _ => AssetKind::Other,
    }
}

pub fn relative_from_root(root: &Path, file: &Path) -> Option<String> {
    file.strip_prefix(root)
        .ok()
        .and_then(|rel| rel.to_str())
        .map(|rel| rel.replace('\\', "/"))
}

pub fn path_is_operational_utf8(path: &Path) -> bool {
    path.to_str().is_some_and(|s| !s.contains('\0'))
}

struct Discovery {
    files: HashSet<PathBuf>,
    bytes_read: u64,
    site_root: PathBuf,
}

impl Discovery {
    fn visit(&mut self, path: PathBuf, kind: AssetKind, depth: usize) -> Result<(), PolicyError> {
        if depth > MAX_DISCOVERY_DEPTH {
            return Err(PolicyError::GraphTooLarge);
        }
        let canonical = match inspect_regular_file(&path) {
            Ok(canonical) => canonical,
            Err(err) if depth == 0 => return Err(err),
            Err(_) => return Ok(()),
        };
        if !is_within_root(&self.site_root, &canonical) {
            return Ok(());
        }
        if !self.files.insert(canonical.clone()) {
            return Ok(());
        }
        if self.files.len() > MAX_DISCOVERY_FILES {
            return Err(PolicyError::GraphTooLarge);
        }
        if !matches!(kind, AssetKind::Html | AssetKind::Css | AssetKind::Js) {
            return Ok(());
        }
        let text = match self.read_text(&canonical) {
            Ok(text) => text,
            Err(err) if depth == 0 => return Err(err),
            Err(_) => return Ok(()),
        };
        let from_dir = canonical
            .parent()
            .ok_or(PolicyError::InvalidPath)?
            .to_path_buf();
        for spec in extract_declared_specs(kind, &text) {
            if let Some(next) = resolve_declared_spec(&from_dir, &self.site_root, &spec) {
                let next_kind = classify_kind(&next);
                self.visit(next, next_kind, depth + 1)?;
            }
        }
        Ok(())
    }

    fn read_text(&mut self, path: &Path) -> Result<String, PolicyError> {
        let meta = fs::metadata(path).map_err(|_| PolicyError::NotAccessible)?;
        let len = meta.len();
        if len > MAX_DISCOVERY_BYTES || self.bytes_read.saturating_add(len) > MAX_DISCOVERY_BYTES {
            return Err(PolicyError::GraphTooLarge);
        }
        let bytes = fs::read(path).map_err(|_| PolicyError::NotAccessible)?;
        self.bytes_read = self.bytes_read.saturating_add(bytes.len() as u64);
        String::from_utf8(bytes).map_err(|_| PolicyError::InvalidPath)
    }
}

fn inspect_selected_html(path: &Path) -> Result<PathBuf, PolicyError> {
    if !path_is_operational_utf8(path) {
        return Err(PolicyError::InvalidPath);
    }
    if is_denied_path(path) {
        return Err(PolicyError::Denied);
    }
    if !matches!(classify_kind(path), AssetKind::Html) {
        return Err(PolicyError::Denied);
    }
    inspect_regular_file(path)
}

fn inspect_regular_file(path: &Path) -> Result<PathBuf, PolicyError> {
    if !path_is_operational_utf8(path) {
        return Err(PolicyError::InvalidPath);
    }
    if raw_path_has_denied_name(path) {
        return Err(PolicyError::Denied);
    }
    let meta = fs::symlink_metadata(path).map_err(|_| PolicyError::NotAccessible)?;
    if meta.file_type().is_symlink() {
        return Err(PolicyError::Denied);
    }
    if !meta.is_file() {
        return Err(PolicyError::NotAFile);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| PolicyError::NotAccessible)?;
    if !path_is_operational_utf8(&canonical) || is_denied_path(&canonical) {
        return Err(PolicyError::Denied);
    }
    let canonical_meta =
        fs::symlink_metadata(&canonical).map_err(|_| PolicyError::NotAccessible)?;
    if canonical_meta.file_type().is_symlink() || !canonical_meta.is_file() {
        return Err(PolicyError::Denied);
    }
    Ok(canonical)
}

fn raw_path_has_denied_name(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => name.to_str().is_none_or(component_is_denied),
        _ => false,
    })
}

fn component_is_denied(name: &str) -> bool {
    if name.is_empty() || name.contains('\0') {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    if VCS_COMPONENTS.contains(&lower.as_str()) {
        return true;
    }
    if DENIED_BASENAMES.contains(&lower.as_str()) {
        return true;
    }
    if let Some((_, ext)) = lower.rsplit_once('.') {
        if DENIED_EXTENSIONS.contains(&ext) {
            return true;
        }
    }
    false
}

fn is_within_root(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

pub fn resolve_declared_spec(from_dir: &Path, site_root: &Path, spec: &str) -> Option<PathBuf> {
    let spec = strip_url_suffix(spec.trim());
    if spec.is_empty() || should_skip_spec(spec) {
        return None;
    }
    let joined = if spec.starts_with('/') {
        join_relative(site_root, site_root, spec.trim_start_matches('/'))?
    } else {
        join_relative(from_dir, site_root, spec)?
    };
    if !path_is_operational_utf8(&joined)
        || is_denied_path(&joined)
        || !is_within_root(site_root, &joined)
    {
        return None;
    }
    Some(joined)
}

fn join_relative(base: &Path, site_root: &Path, spec: &str) -> Option<PathBuf> {
    let mut current = base.to_path_buf();
    for part in spec.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let parent = current.parent()?.to_path_buf();
            if !is_within_root(site_root, &parent) {
                return None;
            }
            current = parent;
            continue;
        }
        if part.contains('\0') || part.starts_with('.') || component_is_denied(part) {
            return None;
        }
        current.push(part);
        if !is_within_root(site_root, &current) {
            return None;
        }
        if fs::symlink_metadata(&current)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
        {
            return None;
        }
    }
    Some(current)
}

fn should_skip_spec(spec: &str) -> bool {
    if spec.is_empty() || spec.starts_with('#') || spec.contains('\0') {
        return true;
    }
    let lower = spec.to_ascii_lowercase();
    if lower.starts_with("//") {
        return true;
    }
    if looks_like_scheme(&lower) {
        return true;
    }
    let bytes = lower.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn looks_like_scheme(spec: &str) -> bool {
    let Some(index) = spec.find(':') else {
        return false;
    };
    let scheme = &spec[..index];
    !scheme.is_empty() && scheme.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn strip_url_suffix(spec: &str) -> &str {
    spec.split_once(['?', '#'])
        .map(|(head, _)| head)
        .unwrap_or(spec)
}

fn extract_declared_specs(kind: AssetKind, text: &str) -> Vec<String> {
    match kind {
        AssetKind::Html => extract_html_specs(text),
        AssetKind::Css => extract_css_urls(text),
        AssetKind::Js => extract_js_module_specifiers(text),
        AssetKind::Other => Vec::new(),
    }
}

fn extract_html_specs(html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("*") else {
        return Vec::new();
    };
    let mut specs = Vec::new();
    for element in document.select(&selector) {
        let name = element.value().name();
        match name {
            "script" | "iframe" | "frame" | "embed" | "img" | "video" | "audio" | "source"
            | "track" => {
                push_attr(&mut specs, element.value().attr("src"));
                push_attr(&mut specs, element.value().attr("poster"));
                push_srcset(&mut specs, element.value().attr("srcset"));
            }
            "link" => {
                let rel = element
                    .value()
                    .attr("rel")
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if rel.split_whitespace().any(|token| {
                    matches!(
                        token,
                        "stylesheet"
                            | "icon"
                            | "shortcut"
                            | "apple-touch-icon"
                            | "preload"
                            | "modulepreload"
                    )
                }) {
                    push_attr(&mut specs, element.value().attr("href"));
                    push_srcset(&mut specs, element.value().attr("imagesrcset"));
                }
            }
            "image" | "use" => {
                push_attr(&mut specs, element.value().attr("href"));
                push_attr(&mut specs, element.value().attr("src"));
                push_attr(&mut specs, element.value().attr("xlink:href"));
            }
            _ => {}
        }
    }
    specs
}

fn push_attr(specs: &mut Vec<String>, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        specs.push(value.to_string());
    }
}

fn push_srcset(specs: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    for candidate in value.split(',') {
        if let Some(url) = candidate.split_whitespace().next() {
            if !url.is_empty() {
                specs.push(url.to_string());
            }
        }
    }
}

fn extract_css_urls(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut specs = Vec::new();
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = index.saturating_add(2);
            continue;
        }
        if bytes[index] == b'"' || bytes[index] == b'\'' {
            let (consumed, _) = read_css_quoted(bytes, index);
            index += consumed;
            continue;
        }
        if match_at_keyword(bytes, index, b"@import") {
            index += 7;
            index = skip_css_ws(bytes, index);
            if let Some((consumed, value)) = read_css_import_target(bytes, index) {
                specs.push(value);
                index += consumed;
                continue;
            }
            continue;
        }
        if match_ident(bytes, index, b"url") && bytes.get(index + 3) == Some(&b'(') {
            index += 4;
            if let Some((consumed, value)) = read_css_url_target(bytes, index) {
                specs.push(value);
                index += consumed;
                continue;
            }
            continue;
        }
        index += 1;
    }
    specs
}

fn read_css_import_target(bytes: &[u8], index: usize) -> Option<(usize, String)> {
    if match_ident(bytes, index, b"url") && bytes.get(index + 3) == Some(&b'(') {
        let inner_start = index + 4;
        let (consumed, value) = read_css_url_target(bytes, inner_start)?;
        return Some((inner_start - index + consumed, value));
    }
    if bytes.get(index) == Some(&b'"') || bytes.get(index) == Some(&b'\'') {
        let (consumed, value) = read_css_quoted(bytes, index);
        return value.map(|value| (consumed, value));
    }
    None
}

fn read_css_url_target(bytes: &[u8], mut index: usize) -> Option<(usize, String)> {
    let start = index;
    index = skip_css_ws(bytes, index);
    let value = if bytes.get(index) == Some(&b'"') || bytes.get(index) == Some(&b'\'') {
        let (consumed, value) = read_css_quoted(bytes, index);
        index += consumed;
        value?
    } else {
        let value_start = index;
        while index < bytes.len() && bytes[index] != b')' && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let raw = std::str::from_utf8(&bytes[value_start..index]).ok()?;
        raw.to_string()
    };
    index = skip_css_ws(bytes, index);
    if bytes.get(index) == Some(&b')') {
        index += 1;
    }
    Some((index - start, value))
}

fn read_css_quoted(bytes: &[u8], index: usize) -> (usize, Option<String>) {
    let quote = bytes[index];
    let mut cursor = index + 1;
    let mut escaped = false;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if escaped {
            escaped = false;
            cursor += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            cursor += 1;
            continue;
        }
        if byte == quote {
            let raw = std::str::from_utf8(&bytes[index + 1..cursor]).ok();
            return (cursor + 1 - index, raw.map(ToOwned::to_owned));
        }
        cursor += 1;
    }
    (cursor - index, None)
}

fn skip_css_ws(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn match_at_keyword(bytes: &[u8], index: usize, needle: &[u8]) -> bool {
    let Some(slice) = bytes.get(index..index + needle.len()) else {
        return false;
    };
    slice.eq_ignore_ascii_case(needle)
        && bytes
            .get(index + needle.len())
            .is_none_or(|next| !is_ident_byte(*next))
}

fn match_ident(bytes: &[u8], index: usize, needle: &[u8]) -> bool {
    let Some(slice) = bytes.get(index..index + needle.len()) else {
        return false;
    };
    slice.eq_ignore_ascii_case(needle)
        && (index == 0 || !is_ident_byte(bytes[index - 1]))
        && bytes
            .get(index + needle.len())
            .is_none_or(|next| !is_ident_byte(*next))
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn extract_js_module_specifiers(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut index = 0;
    let mut specs = Vec::new();
    while index < chars.len() {
        if chars[index] == '/' && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = index.saturating_add(2);
            continue;
        }
        if chars[index] == '\'' || chars[index] == '"' {
            index = skip_js_string(&chars, index);
            continue;
        }
        if chars[index] == '`' {
            index = skip_js_template(&chars, index);
            continue;
        }
        if is_js_ident_start(chars[index]) {
            let (word, next) = read_js_ident(&chars, index);
            if word == "import" || word == "export" {
                index = skip_js_ws_and_comments(&chars, next);
                if word == "import" && chars.get(index) == Some(&'.') {
                    index += 1;
                    continue;
                }
                if word == "import" && chars.get(index) == Some(&'(') {
                    index = skip_js_ws_and_comments(&chars, index + 1);
                    if let Some((value, next_index)) = read_js_string_at(&chars, index) {
                        specs.push(value);
                        index = next_index;
                    }
                    continue;
                }
                let scan_end = scan_js_statement_end(&chars, index);
                collect_js_strings(&chars, index, scan_end, &mut specs);
                index = scan_end;
                continue;
            }
            index = next;
            continue;
        }
        index += 1;
    }
    specs
}

fn skip_js_ws_and_comments(chars: &[char], mut index: usize) -> usize {
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = index.saturating_add(2);
            continue;
        }
        break;
    }
    index
}

fn scan_js_statement_end(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() && chars[index] != ';' && chars[index] != '\n' {
        if chars[index] == '\'' || chars[index] == '"' {
            index = skip_js_string(chars, index);
            continue;
        }
        if chars[index] == '`' {
            index = skip_js_template(chars, index);
            continue;
        }
        index += 1;
    }
    if index < chars.len() {
        index + 1
    } else {
        index
    }
}

fn collect_js_strings(chars: &[char], mut index: usize, end: usize, specs: &mut Vec<String>) {
    while index < end {
        if chars[index] == '\'' || chars[index] == '"' {
            if let Some((value, next)) = read_js_string_at(chars, index) {
                specs.push(value);
                index = next;
                continue;
            }
        }
        if chars[index] == '`' {
            index = skip_js_template(chars, index);
            continue;
        }
        index += 1;
    }
}

fn read_js_ident(chars: &[char], start: usize) -> (String, usize) {
    let mut index = start + 1;
    while index < chars.len() && is_js_ident_continue(chars[index]) {
        index += 1;
    }
    (chars[start..index].iter().collect(), index)
}

fn is_js_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_js_ident_continue(ch: char) -> bool {
    is_js_ident_start(ch) || ch.is_ascii_digit()
}

fn read_js_string_at(chars: &[char], index: usize) -> Option<(String, usize)> {
    let quote = *chars.get(index)?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut cursor = index + 1;
    let mut escaped = false;
    let mut out = String::new();
    while cursor < chars.len() {
        let ch = chars[cursor];
        if escaped {
            out.push(ch);
            escaped = false;
            cursor += 1;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            cursor += 1;
            continue;
        }
        if ch == quote {
            return Some((out, cursor + 1));
        }
        out.push(ch);
        cursor += 1;
    }
    None
}

fn skip_js_string(chars: &[char], index: usize) -> usize {
    read_js_string_at(chars, index)
        .map(|(_, next)| next)
        .unwrap_or(chars.len())
}

fn skip_js_template(chars: &[char], index: usize) -> usize {
    let mut cursor = index + 1;
    let mut escaped = false;
    while cursor < chars.len() {
        let ch = chars[cursor];
        if escaped {
            escaped = false;
            cursor += 1;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            cursor += 1;
            continue;
        }
        if ch == '`' {
            return cursor + 1;
        }
        cursor += 1;
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn denies_secret_and_dotfile_names() {
        assert!(is_denied_path(Path::new("/ws/.env")));
        assert!(is_denied_path(Path::new("/ws/.git/config")));
        assert!(is_denied_path(Path::new("/ws/id_rsa")));
        assert!(is_denied_path(Path::new("/ws/secrets.json")));
        assert!(is_denied_path(Path::new("/ws/app.env")));
        assert!(!is_denied_path(Path::new("/ws/index.html")));
        assert!(!is_denied_path(Path::new("/ws/css/app.css")));
    }

    #[test]
    fn extracts_html_css_and_js_specs() {
        let html = r#"
            <link rel="stylesheet" href="app.css">
            <script type="module" src="app.js"></script>
            <img src="logo.png" srcset="logo.png 1x, logo@2x.png 2x">
        "#;
        let specs = extract_html_specs(html);
        assert!(specs.contains(&"app.css".into()));
        assert!(specs.contains(&"app.js".into()));
        assert!(specs.contains(&"logo.png".into()));
        assert!(specs.contains(&"logo@2x.png".into()));

        let css = r#"
            /* url(hidden.css) */
            @import "more.css";
            body { background: url(bg.png); }
        "#;
        let css_specs = extract_css_urls(css);
        assert!(css_specs.contains(&"more.css".into()));
        assert!(css_specs.contains(&"bg.png".into()));
        assert!(!css_specs.iter().any(|spec| spec.contains("hidden.css")));

        let js = r#"
            import { x } from "./mod.js";
            export * from "../shared.js";
            import("./lazy.js");
            const skip = "not-an-import";
        "#;
        let js_specs = extract_js_module_specifiers(js);
        assert!(js_specs.contains(&"./mod.js".into()));
        assert!(js_specs.contains(&"../shared.js".into()));
        assert!(js_specs.contains(&"./lazy.js".into()));
        assert!(!js_specs.iter().any(|spec| spec == "not-an-import"));
    }

    #[test]
    fn allowlist_includes_declared_assets_only() {
        let root = tempfile::tempdir().unwrap();
        write_file(
            &root.path().join("index.html"),
            r#"<link rel="stylesheet" href="app.css"><script src="app.js"></script><img src="logo.png">"#,
        );
        write_file(&root.path().join("app.css"), r#"@import url("more.css");"#);
        write_file(&root.path().join("more.css"), "body{color:red}");
        write_file(
            &root.path().join("app.js"),
            r#"import { x } from "./mod.js";"#,
        );
        write_file(&root.path().join("mod.js"), "export const x = 1;");
        write_file(&root.path().join("logo.png"), "png");
        write_file(&root.path().join("secret.txt"), "nope");
        write_file(&root.path().join(".env"), "SECRET=1");

        let allowlist = build_allowlist(&root.path().join("index.html")).unwrap();
        let names: HashSet<String> = allowlist
            .files
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(ToOwned::to_owned))
            .collect();
        assert!(names.contains("index.html"));
        assert!(names.contains("app.css"));
        assert!(names.contains("more.css"));
        assert!(names.contains("app.js"));
        assert!(names.contains("mod.js"));
        assert!(names.contains("logo.png"));
        assert!(!names.contains("secret.txt"));
        assert!(!names.contains(".env"));
    }

    #[test]
    fn parent_traversal_html_css_js_cannot_expand_preview_root() {
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        let secret = root.path().join("private.pdf");
        write_file(&secret, "secret");
        write_file(
            &site.join("index.html"),
            r#"<link rel="stylesheet" href="app.css"><script src="app.js"></script><img src="../private.pdf">"#,
        );
        write_file(
            &site.join("app.css"),
            r#"body{background:url("../private.pdf")}"#,
        );
        write_file(
            &site.join("app.js"),
            r#"import "../private.pdf"; import { x } from "./ok.js";"#,
        );
        write_file(&site.join("ok.js"), "export const x = 1;");

        let allowlist = build_allowlist(&site.join("index.html")).unwrap();
        assert_eq!(allowlist.url_root, site.canonicalize().unwrap());
        let names: HashSet<String> = allowlist
            .files
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(ToOwned::to_owned))
            .collect();
        assert!(names.contains("index.html"));
        assert!(names.contains("app.css"));
        assert!(names.contains("app.js"));
        assert!(names.contains("ok.js"));
        assert!(!names.contains("private.pdf"));
        assert!(allowlist
            .files
            .iter()
            .all(|path| path.starts_with(&allowlist.url_root)));
    }

    #[cfg(unix)]
    #[test]
    fn directory_swap_and_symlink_escape_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        let outside = root.path().join("outside");
        write_file(&outside.join("stolen.png"), "png");
        write_file(&site.join("index.html"), r#"<img src="assets/stolen.png">"#);
        write_file(&site.join("assets").join("stolen.png"), "inside");
        let allowlist = build_allowlist(&site.join("index.html")).unwrap();
        assert!(allowlist
            .files
            .iter()
            .any(|path| path.ends_with("stolen.png")));

        std::fs::remove_dir_all(site.join("assets")).unwrap();
        std::os::unix::fs::symlink(&outside, site.join("assets")).unwrap();
        let swapped = build_allowlist(&site.join("index.html")).unwrap();
        assert!(!swapped
            .files
            .iter()
            .any(|path| path.ends_with("stolen.png")));
        assert!(swapped
            .files
            .iter()
            .all(|path| path.starts_with(&swapped.url_root)));
    }
}
