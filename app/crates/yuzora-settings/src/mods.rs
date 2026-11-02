//! The person's mods and layouts in the app home, as the MOD page lists them:
//! which are shipped (the app installs and replaces them), which of those
//! were edited in place, which are clones (`cloned_from`), which are theirs.
//!
//! Herdr records what it installed in `<home>/installed.json`: per shipped
//! mod, its files and an FNV-1a fingerprint of each; per shipped layout a
//! `layout:<name>` key. A shipped mod whose files no longer match (or that
//! gained a file) is edited: the next start moves it to `<name>-edited` and
//! puts the shipped one back. [`reset_mod`] does that now, when the app has
//! handed over its shipped files ([`set_shipped_mods`]).
//!
//! Installing from a folder copies a mod folder (one with `main.js`) or a
//! shared layout (a folder with `layouts/` and `mods/`) into the home. It
//! never overwrites: a taken name gets `-2`, `-3`…, and the paths and `mod`
//! names that pointed at the old name are rewritten, as `mod.clone` does.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Files = BTreeMap<String, String>;

/// Folders bigger than this aren't a mod someone meant to install.
const MAX_FILES: usize = 2000;
const MAX_BYTES: u64 = 64 << 20;

/// FNV-1a, as Herdr fingerprints installed files.
pub fn fingerprint(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    format!("{h:016x}")
}

/// `installed.json`: shipped mod name (or `layout:<name>`) to its files.
pub fn installed(home: &Path) -> BTreeMap<String, Files> {
    std::fs::read_to_string(home.join("installed.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Exactly as installed: every file unchanged and none added (the editor
/// helpers gpui-shell writes on load don't count), as Herdr decides it.
pub fn as_shipped(dir: &Path, files: &Files) -> bool {
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found.retain(|f| !matches!(f.as_str(), "gpui-kit.d.ts" | "jsconfig.json") || files.contains_key(f));
    found.len() == files.len()
        && found.iter().all(|f| files.contains_key(f))
        && files.iter().all(|(path, want)| std::fs::read(dir.join(path)).is_ok_and(|b| fingerprint(&b) == *want))
}

/// The shipped mods' files, `("<mod>/<path>", bytes)`, as the app embeds them.
pub type Shipped = &'static [(&'static str, &'static [u8])];

static SHIPPED: OnceLock<Shipped> = OnceLock::new();

/// The app hands over its shipped mods once, so [`reset_mod`] can put one back.
pub fn set_shipped_mods(files: Shipped) {
    let _ = SHIPPED.set(files);
}

fn shipped_files(name: &str) -> Option<Vec<(&'static str, &'static [u8])>> {
    let prefix = format!("{name}/");
    let files: Vec<_> = SHIPPED.get()?.iter().filter_map(|(p, b)| Some((p.strip_prefix(&prefix)?, *b))).collect();
    (!files.is_empty()).then_some(files)
}

/// Whether [`reset_mod`] can put this mod back now.
pub fn can_reset(name: &str) -> bool {
    shipped_files(name).is_some()
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn cloned_from(v: &Value) -> Value {
    match v.get("cloned_from") {
        Some(c) if c.is_object() => json!({ "name": c.get("name"), "version": c.get("version") }),
        _ => Value::Null,
    }
}

/// Every mod folder in `<home>/mods`, by name.
pub fn list_mods(home: &Path) -> Vec<Value> {
    let installed = installed(home);
    let mods = home.join("mods");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&mods).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|dir| {
            let name = dir.file_name()?.to_str()?.to_owned();
            if name.starts_with('.') {
                return None;
            }
            let meta = read_json(&dir.join("mod.json")).unwrap_or(Value::Null);
            let record = installed.get(&name);
            let shipped = record.is_some();
            let edited = record.is_some_and(|files| !as_shipped(&dir, files));
            let yz = meta.get("yuzora");
            Some(json!({
                "name": name,
                "path": dir.to_string_lossy(),
                "title": meta.get("title").and_then(Value::as_str),
                "place": meta.get("place").and_then(Value::as_str),
                "main": dir.join("main.js").is_file(),
                "manifest": dir.join("mod.json").is_file(),
                "shipped": shipped,
                "edited": edited,
                "resettable": shipped && can_reset(&name),
                "cloned_from": cloned_from(&meta),
                "pane": yz.and_then(|y| y.get("pane")).is_some(),
                "settings": yz.and_then(|y| y.get("settings")).is_some(),
                "source": if !shipped { if cloned_from(&meta).is_null() { "personal" } else { "clone" } } else if edited { "edited" } else { "shipped" },
            }))
        })
        .collect()
}

/// Every layout in `<home>/layouts`, by name.
pub fn list_layouts(home: &Path) -> Vec<Value> {
    let installed = installed(home);
    let dir = home.join("layouts");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "json") && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_owned();
            let root = read_json(&path);
            let shipped = installed.contains_key(&format!("layout:{name}"));
            let cloned = root.as_ref().map(cloned_from).unwrap_or(Value::Null);
            Some(json!({
                "name": name,
                "path": path.to_string_lossy(),
                "valid": root.is_some(),
                "shipped": shipped,
                "cloned_from": cloned,
                "preview": root.as_ref().and_then(|r| r.get("preview")).and_then(Value::as_str),
                "source": if shipped { "shipped" } else if cloned.is_null() { "personal" } else { "clone" },
            }))
        })
        .collect()
}

fn free_name(taken: impl Fn(&str) -> bool, base: &str) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| !taken(n)).unwrap()
}

/// Puts a shipped mod back as the app ships it. An edited one first moves to
/// `<name>-edited` (as the next start would). Returns that clone's name.
pub fn reset_mod(home: &Path, name: &str) -> std::io::Result<Option<String>> {
    let files = shipped_files(name).ok_or_else(|| std::io::Error::other("not a shipped mod"))?;
    put_back(home, name, &files)
}

fn put_back(home: &Path, name: &str, files: &[(&str, &[u8])]) -> std::io::Result<Option<String>> {
    let mods = home.join("mods");
    let target = mods.join(name);
    let mut kept = None;
    if target.exists() {
        let recorded = installed(home).get(name).cloned().unwrap_or_else(|| files.iter().map(|(p, b)| ((*p).to_owned(), fingerprint(b))).collect());
        if as_shipped(&target, &recorded) {
            std::fs::remove_dir_all(&target)?;
        } else {
            let clone = (1..)
                .map(|i| if i == 1 { format!("{name}-edited") } else { format!("{name}-edited-{i}") })
                .find(|n| !mods.join(n).exists())
                .unwrap();
            std::fs::rename(&target, mods.join(&clone))?;
            kept = Some(clone);
        }
    }
    for (path, bytes) in files {
        let file = target.join(path);
        std::fs::create_dir_all(file.parent().unwrap_or(&target))?;
        std::fs::write(file, bytes)?;
    }
    Ok(kept)
}

/// Text that named mod `from` now names `to`: asset paths and `mod:` keys,
/// the same rewrites `mod.clone` makes.
pub fn rewrite(text: &str, from: &str, to: &str) -> String {
    let mut text = text
        .replace(&format!("mods/{from}/"), &format!("mods/{to}/"))
        .replace(&format!("\"mods/{from}\""), &format!("\"mods/{to}\""))
        .replace(&format!("'mods/{from}'"), &format!("'mods/{to}'"));
    for key in ["mod", "\"mod\"", "'mod'"] {
        for space in ["", " "] {
            for quote in ["\"", "'"] {
                text = text.replace(&format!("{key}:{space}{quote}{from}{quote}"), &format!("{key}:{space}{quote}{to}{quote}"));
            }
        }
    }
    text
}

/// What installing a folder would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// `mod` (one mod folder) or `bundle` (`layouts/` and `mods/`).
    pub kind: &'static str,
    pub source: PathBuf,
    /// `(folder, installed as)`.
    pub mods: Vec<(PathBuf, String)>,
    pub layouts: Vec<(PathBuf, String)>,
    /// A `keys.json` came with it (merged by hand, not here).
    pub keys: bool,
    /// What its code reaches: `programs`, `network`, `files`.
    pub reaches: Vec<&'static str>,
    pub files: usize,
    pub bytes: u64,
}

impl Plan {
    pub fn to_json(&self) -> Value {
        let pairs = |l: &[(PathBuf, String)]| {
            l.iter()
                .map(|(p, to)| json!({ "from": p.file_name().map(|n| n.to_string_lossy().trim_end_matches(".json").to_owned()), "as": to }))
                .collect::<Vec<_>>()
        };
        json!({
            "kind": self.kind,
            "source": self.source.to_string_lossy(),
            "mods": pairs(&self.mods),
            "layouts": pairs(&self.layouts),
            "keys": self.keys,
            "reaches": self.reaches,
            "files": self.files,
            "bytes": self.bytes,
        })
    }
}

#[derive(Debug)]
pub enum InstallError {
    /// Nothing to install there: no `main.js`, no `mods/` or `layouts/`.
    NotAMod,
    /// It's already in the home.
    Inside,
    TooBig,
    Io(std::io::Error),
}

impl InstallError {
    pub fn code(&self) -> &'static str {
        match self {
            InstallError::NotAMod => "not_a_mod",
            InstallError::Inside => "inside_home",
            InstallError::TooBig => "too_big",
            InstallError::Io(_) => "io",
        }
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Io(e) => write!(f, "io: {e}"),
            other => f.write_str(other.code()),
        }
    }
}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

/// Left out of a copy: version control, dependencies, the share page's own files.
fn skipped(name: &str, at_root: bool) -> bool {
    name.starts_with('.') || name == "node_modules" || (at_root && matches!(name, "RECIPE.md" | "preview.png"))
}

fn files_in(dir: &Path, root: bool, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if skipped(&name, root) {
            continue;
        }
        let ty = e.file_type()?;
        if ty.is_dir() {
            files_in(&e.path(), false, out)?;
        } else if ty.is_file() {
            out.push(e.path());
        }
        if out.len() > MAX_FILES {
            return Err(std::io::Error::other("too many files"));
        }
    }
    Ok(())
}

/// A folder name as a mod name: anything but letters, digits, `-` and `_` becomes `-`.
fn mod_name(dir: &Path) -> String {
    let raw = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if name.trim_matches('-').is_empty() { "mod".to_owned() } else { name }
}

/// Reads a folder and decides where each piece goes, without writing anything.
pub fn plan(source: &Path, home: &Path) -> Result<Plan, InstallError> {
    let source = std::fs::canonicalize(source)?;
    let home_real = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if source.starts_with(&home_real) {
        return Err(InstallError::Inside);
    }
    let mut plan = Plan { source: source.clone(), ..Plan::default() };
    let mod_dirs: Vec<PathBuf> = if source.join("main.js").is_file() {
        plan.kind = "mod";
        vec![source.clone()]
    } else {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(source.join("mods"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("main.js").is_file())
            .collect();
        dirs.sort();
        let mut layouts: Vec<PathBuf> = std::fs::read_dir(source.join("layouts"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "json"))
            .collect();
        layouts.sort();
        if dirs.is_empty() && layouts.is_empty() {
            return Err(InstallError::NotAMod);
        }
        plan.kind = "bundle";
        plan.keys = source.join("keys.json").is_file();
        let taken = |n: &str| home.join("layouts").join(format!("{n}.json")).exists();
        let mut chosen: Vec<String> = Vec::new();
        for l in layouts {
            let base = l.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name = free_name(|n| taken(n) || chosen.iter().any(|c| c == n), &base);
            chosen.push(name.clone());
            plan.layouts.push((l, name));
        }
        dirs
    };
    let mut chosen: Vec<String> = Vec::new();
    for dir in mod_dirs {
        let base = mod_name(&dir);
        let name = free_name(|n| home.join("mods").join(n).exists() || chosen.iter().any(|c| c == n), &base);
        chosen.push(name.clone());
        plan.mods.push((dir, name));
    }
    let mut files = Vec::new();
    for (dir, _) in &plan.mods {
        files_in(dir, true, &mut files).map_err(|_| InstallError::TooBig)?;
    }
    files.extend(plan.layouts.iter().map(|(p, _)| p.clone()));
    plan.files = files.len();
    let mut reaches = std::collections::BTreeSet::new();
    for f in &files {
        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        plan.bytes += size;
        if plan.files > MAX_FILES || plan.bytes > MAX_BYTES {
            return Err(InstallError::TooBig);
        }
        let is_code = f.extension().is_some_and(|e| matches!(e.to_str(), Some("js" | "mjs" | "ts")));
        if is_code && let Ok(text) = std::fs::read_to_string(f) {
            reaches.extend(reach(&text));
        }
    }
    plan.reaches = reaches.into_iter().collect();
    Ok(plan)
}

/// What a piece of mod code can do beyond drawing (a hint for the warning,
/// not a guarantee: mods are trusted code and may do anything).
pub fn reach(code: &str) -> Vec<&'static str> {
    let any = |needles: &[&str]| needles.iter().any(|n| code.contains(n));
    let mut out = Vec::new();
    if any(&["exec(", "hc(", "\"process\"", "'process'", "knownAgents("]) {
        out.push("programs");
    }
    if any(&["fetch(", "\"websocket\"", "'websocket'", "\"net\"", "'net'", "WebSocket"]) {
        out.push("network");
    }
    if any(&["readTextFile(", "writeTextFile(", "fs/promises", "watchFolder("]) {
        out.push("files");
    }
    out
}

fn copy_dir(from: &Path, to: &Path, root: bool, renames: &[(String, String)]) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if skipped(&name, root) {
            continue;
        }
        let ty = e.file_type()?;
        let dest = to.join(&name);
        if ty.is_dir() {
            copy_dir(&e.path(), &dest, false, renames)?;
        } else if ty.is_file() {
            copy_file(&e.path(), &dest, renames)?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path, renames: &[(String, String)]) -> std::io::Result<()> {
    let bytes = std::fs::read(from)?;
    match std::str::from_utf8(&bytes) {
        Ok(text) if !renames.is_empty() => std::fs::write(to, renames.iter().fold(text.to_owned(), |t, (a, b)| rewrite(&t, a, b))),
        _ => std::fs::write(to, bytes),
    }
}

/// Installs what [`plan`] decides (planned again, so nothing changed since).
/// A failure removes what this install wrote.
pub fn install(source: &Path, home: &Path) -> Result<Plan, InstallError> {
    let plan = plan(source, home)?;
    let renames: Vec<(String, String)> = plan.mods.iter().map(|(d, to)| (mod_name(d), to.clone())).filter(|(a, b)| a != b).collect();
    let mut written: Vec<PathBuf> = Vec::new();
    let result = (|| -> std::io::Result<()> {
        for (dir, to) in &plan.mods {
            let target = home.join("mods").join(to);
            if target.exists() {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{to} appeared meanwhile")));
            }
            written.push(target.clone());
            let own: Vec<(String, String)> = renames.iter().filter(|(a, _)| *a == mod_name(dir)).cloned().collect();
            copy_dir(dir, &target, true, &own)?;
        }
        std::fs::create_dir_all(home.join("layouts"))?;
        for (file, to) in &plan.layouts {
            let target = home.join("layouts").join(format!("{to}.json"));
            if target.exists() {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{to}.json appeared meanwhile")));
            }
            written.push(target.clone());
            copy_file(file, &target, &renames)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        for p in written {
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
        return Err(e.into());
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yuzora-mods-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn tells_shipped_edited_clone_and_personal_apart() {
        let home = tmp("list");
        write(&home.join("mods/agents/main.js"), "a");
        write(&home.join("mods/tabs/main.js"), "t");
        write(&home.join("mods/tabs/jsconfig.json"), "{}");
        write(&home.join("mods/agents-copy/main.js"), "a");
        write(&home.join("mods/agents-copy/mod.json"), r#"{"place":"left","cloned_from":{"name":"agents","version":"0.1.0-beta.2"}}"#);
        write(&home.join("mods/mine/main.js"), "m");
        write(&home.join("mods/mine/mod.json"), r#"{"title":"Mine","yuzora":{"settings":{"title":"t"}}}"#);
        let files = |text: &str| -> Files { [("main.js".to_owned(), fingerprint(text.as_bytes()))].into() };
        let record: BTreeMap<String, Files> = [("agents".to_owned(), files("old")), ("tabs".to_owned(), files("t")), ("layout:main".to_owned(), Files::new())].into();
        write(&home.join("installed.json"), &serde_json::to_string(&record).unwrap());
        write(&home.join("layouts/main.json"), r#"{"id":"root","type":"block","preview":"layouts/main.png"}"#);
        write(&home.join("layouts/mine.json"), r#"{"id":"root","type":"block","cloned_from":{"name":"main","version":"x"}}"#);
        write(&home.join("layouts/broken.json"), "{");
        write(&home.join("layouts/main.json.before-0.1.0"), "{}");

        let mods = list_mods(&home);
        let row = |n: &str| mods.iter().find(|m| m["name"] == n).unwrap().clone();
        assert_eq!(row("agents")["source"], "edited", "its main.js no longer matches");
        assert_eq!(row("tabs")["source"], "shipped", "jsconfig.json is gpui-shell's, not an edit");
        assert_eq!(row("agents-copy")["source"], "clone");
        assert_eq!(row("agents-copy")["cloned_from"]["name"], "agents");
        assert_eq!(row("mine")["source"], "personal");
        assert_eq!(row("mine")["settings"], true);
        assert_eq!(row("mine")["title"], "Mine");
        assert_eq!(row("tabs")["resettable"], false, "no shipped files handed over in tests");

        let layouts = list_layouts(&home);
        let names: Vec<_> = layouts.iter().map(|l| (l["name"].as_str().unwrap(), l["source"].as_str().unwrap(), l["valid"].as_bool().unwrap())).collect();
        assert_eq!(names, [("broken", "personal", false), ("main", "shipped", true), ("mine", "clone", true)]);
        assert_eq!(layouts[1]["preview"], "layouts/main.png");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn an_added_file_is_an_edit() {
        let d = tmp("edit");
        write(&d.join("main.js"), "x");
        let files: Files = [("main.js".to_owned(), fingerprint(b"x"))].into();
        assert!(as_shipped(&d, &files));
        write(&d.join(".hidden"), "ignored");
        assert!(as_shipped(&d, &files));
        write(&d.join("extra.js"), "y");
        assert!(!as_shipped(&d, &files));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rewrites_asset_paths_and_mod_names() {
        let text = r#"svg("mods/old/icon.svg"); run("mod.open", {name: "old"}); const x = { mod: 'old' }; {"mod": "old", "icon": "mods/old"} oldish"#;
        assert_eq!(
            rewrite(text, "old", "new"),
            r#"svg("mods/new/icon.svg"); run("mod.open", {name: "old"}); const x = { mod: 'new' }; {"mod": "new", "icon": "mods/new"} oldish"#
        );
    }

    #[test]
    fn reads_what_code_reaches() {
        assert_eq!(reach("import { exec } from 'herdr'; await exec('git', [])"), ["programs"]);
        assert_eq!(reach("await fetch('https://x')"), ["network"]);
        assert_eq!(reach("await readTextFile(p)"), ["files"]);
        assert!(reach("div().child('hi')").is_empty());
    }

    #[test]
    fn installs_a_mod_folder_beside_a_taken_name() {
        let root = tmp("install-mod");
        let home = root.join("home");
        write(&home.join("mods/board/main.js"), "mine");
        let src = root.join("share/board");
        write(&src.join("main.js"), r#"svg("mods/board/icon.svg"); exec("git")"#);
        write(&src.join("icon.svg"), "<svg/>");
        write(&src.join("RECIPE.md"), "brief");
        write(&src.join("preview.png"), "png");
        write(&src.join(".git/HEAD"), "ref");

        let p = plan(&src, &home).unwrap();
        assert_eq!(p.kind, "mod");
        assert_eq!(p.mods.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(), ["board-2"]);
        assert_eq!(p.reaches, ["programs"]);
        assert_eq!(p.files, 2, "the recipe, preview and .git stay out");
        assert!(!home.join("mods/board-2").exists(), "planning writes nothing");

        install(&src, &home).unwrap();
        assert_eq!(std::fs::read_to_string(home.join("mods/board/main.js")).unwrap(), "mine", "never overwrites");
        assert_eq!(std::fs::read_to_string(home.join("mods/board-2/main.js")).unwrap(), r#"svg("mods/board-2/icon.svg"); exec("git")"#);
        assert!(home.join("mods/board-2/icon.svg").exists());
        assert!(!home.join("mods/board-2/RECIPE.md").exists() && !home.join("mods/board-2/.git").exists());
        assert!(matches!(plan(&home.join("mods/board"), &home), Err(InstallError::Inside)));
        assert!(matches!(plan(&root.join("share"), &home), Err(InstallError::NotAMod)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn installs_a_shared_layout_and_points_it_at_renamed_mods() {
        let root = tmp("install-bundle");
        let home = root.join("home");
        write(&home.join("mods/side/main.js"), "mine");
        write(&home.join("layouts/deck.json"), "{}");
        let src = root.join("share");
        write(&src.join("layouts/deck.json"), r#"{"type":"block","content":{"mod":"side"},"preview":"x"}"#);
        write(&src.join("mods/side/main.js"), r#"image("mods/side/a.png")"#);
        write(&src.join("mods/bar/main.js"), "b");
        write(&src.join("keys.json"), "{}");
        write(&src.join("RECIPE.md"), "brief");

        let p = install(&src, &home).unwrap();
        assert_eq!(p.kind, "bundle");
        assert!(p.keys);
        assert_eq!(p.to_json()["layouts"], json!([{ "from": "deck", "as": "deck-2" }]));
        assert_eq!(p.to_json()["mods"], json!([{ "from": "bar", "as": "bar" }, { "from": "side", "as": "side-2" }]));
        assert_eq!(std::fs::read_to_string(home.join("layouts/deck-2.json")).unwrap(), r#"{"type":"block","content":{"mod":"side-2"},"preview":"x"}"#);
        assert_eq!(std::fs::read_to_string(home.join("mods/side-2/main.js")).unwrap(), r#"image("mods/side-2/a.png")"#);
        assert_eq!(std::fs::read_to_string(home.join("layouts/deck.json")).unwrap(), "{}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reset_keeps_edits_aside_and_puts_the_shipped_files_back() {
        let home = tmp("reset");
        assert!(!can_reset("agents"));
        assert!(reset_mod(&home, "agents").is_err(), "no shipped files handed over in tests");
        let shipped: &[(&str, &[u8])] = &[("main.js", b"shipped"), ("icons/a.svg", b"<svg/>")];
        let record: BTreeMap<String, Files> = [("agents".to_owned(), shipped.iter().map(|(p, b)| ((*p).to_owned(), fingerprint(b))).collect())].into();
        write(&home.join("installed.json"), &serde_json::to_string(&record).unwrap());
        write(&home.join("mods/agents/main.js"), "edited");
        write(&home.join("mods/agents-edited/main.js"), "an older rescue");
        assert_eq!(put_back(&home, "agents", shipped).unwrap().as_deref(), Some("agents-edited-2"));
        assert_eq!(std::fs::read_to_string(home.join("mods/agents/main.js")).unwrap(), "shipped");
        assert_eq!(std::fs::read_to_string(home.join("mods/agents/icons/a.svg")).unwrap(), "<svg/>");
        assert_eq!(std::fs::read_to_string(home.join("mods/agents-edited-2/main.js")).unwrap(), "edited");
        assert_eq!(list_mods(&home).iter().find(|m| m["name"] == "agents").unwrap()["source"], "shipped");
        // Untouched: replaced in place, nothing set aside.
        assert_eq!(put_back(&home, "agents", shipped).unwrap(), None);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
