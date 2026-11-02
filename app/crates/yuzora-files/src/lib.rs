//! Yuzora L5（檔案＋編輯器）: Files and the editor: the file tree, search, watchers, code editor and Markdown preview.
//!
//! Owned by lane L5. Scripts reach it as `import { … } from "yuzora:files"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].
//!
//! - [`fsops`], [`git`], [`search`]: files here or on a machine (`hc exec --on`).
//! - [`highlight`]: tree-sitter highlighting, reusable by other crates.
//! - [`editor`]: the reusable code editor (state + frame).
//! - [`docs`]: open documents, the `Editor` component.
//! - [`markdown`], [`image`]: the `Markdown` and `ImageView` components.

pub mod docs;
pub mod editor;
pub mod fsops;
pub mod git;
pub mod highlight;
pub mod image;
pub mod markdown;
pub mod search;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use gpui::{App, ClipboardItem};
use yuzora_core::host::{error, opt_str, to_host};
use yuzora_core::i18n::Catalog;
use yuzora_core::{HostArguments, HostError, HostModule, HostObject, HostValue};

use crate::docs::{DocKey, blocking};
use crate::highlight::Lang;

/// The import specifier: `yuzora:files`.
pub const NAME: &str = "yuzora:files";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r##"
type On = { on?: string | null };
export interface FileEntry { name: string; dir: boolean; link: boolean; hidden: boolean; size: number }
export interface Language { id: string; name: string; badge: string; color: string }
export interface GitMarks { top: string; branch: string | null; detached: boolean; ahead: number; behind: number;
  /** Absolute path → "M" | "A" | "D" | "U" | "?". */
  marks: Record<string, string>; truncated: boolean }
export interface GrepHit { path: string; line: number; column: number; text: string; ranges: [number, number][] }
export interface DocumentState {
  doc: number; seq: number; path: string; on: string | null; name: string; dir: string;
  phase: "loading" | "ready" | "failed"; error: string | null;
  language: string | null; languageName: string | null; badge: string | null; badgeColor: string | null;
  eol: "LF" | "CRLF"; bom: boolean; size: number;
  binary: boolean; tooLarge: boolean; large: boolean; notUtf8: boolean; readonly: boolean; remote: boolean;
  dirty: boolean; saving: boolean; conflict: boolean; deleted: boolean;
  /** When someone else last changed the file on disk (ms since the epoch). */
  externalAt: number | null;
  focused: boolean; owner: string | null; pane: string | null;
  line: number; column: number; matches: number; match: number | null;
}
export interface EditorLook { font_family?: string; font_size?: number;
  colors?: { foreground?: string; muted?: string; background?: string; active_line?: string; selection?: string; caret?: string; border?: string; status?: string };
  /** Semantic name → "#rrggbb" (keyword, string, comment, function, type, …). */
  syntax?: Record<string, string> }

/** The yuzora-files crate's version (Yuzora's). */
export function version(): string;
/** When the app started (ms since the epoch): something recorded before it was from an earlier run. */
export function startedAt(): number;
/** The language a file is highlighted as, by its name. */
export function languageFor(path: string): Language | null;
export function languages(): Language[];
/** Which pane opens a file: the image viewer, the Markdown preview, or the editor. */
export function kindOf(path: string): "image" | "markdown" | "text";

/** A folder's entries, folders first. Skips .git and .DS_Store. */
export function listDir(path: string, options?: On & { limit?: number }): Promise<{ entries: FileEntry[]; truncated: boolean }>;
/** git status for the repo `cwd` is in; null outside a repo. Writes nothing under .git. */
export function gitStatus(cwd: string, options?: On): Promise<GitMarks | null>;
export function stat(path: string, options?: On): Promise<{ len: number; mtime_ms: number } | null>;
/** New file, new folder, rename: this machine only for now (a machine in `on` is refused). */
export function createFile(path: string, options?: On): Promise<null>;
export function createFolder(path: string, options?: On): Promise<null>;
export function rename(from: string, to: string, options?: On): Promise<null>;
/** Moves a file or folder to the system's trash (this machine only). */
export function trash(path: string): Promise<null>;
/** Shows it in Finder / the file manager / Explorer (this machine only). */
export function reveal(path: string): null;
export function copyText(text: string): null;
/** Fuzzy file-name search under root (.gitignore honored); the listing is cached for 15 s.
 *  `truncated`: only the first 200,000 files were searched. `unfiltered`: the machine has
 *  neither git nor ripgrep, so ignored files are included. */
export function findFiles(root: string, query: string, options?: On & { hidden?: boolean; limit?: number; fresh?: boolean }): Promise<{ matches: { path: string; score: number; indices: number[] }[]; truncated: boolean; unfiltered: boolean }>;
/** Content search: ripgrep where it is installed, else a built-in search on this machine; `available` is false on another machine without rg. */
export function grep(root: string, query: string, options?: On & { regex?: boolean; caseSensitive?: boolean; hidden?: boolean; limit?: number }): Promise<{ available: boolean; hits: GrepHit[]; truncated: boolean; files: number }>;

/** Opens (or shares) the document for a file and starts loading it. Pair with releaseDocument. */
export function openDocument(path: string, options?: On): number;
/** Drops one reference; `owner` gives up the editor. Unsaved documents stay in memory. */
export function releaseDocument(doc: number, owner?: string | null): null;
export function documentState(doc: number): DocumentState | null;
/** The document already open for a file, and the pane last seen showing it. */
export function documentFor(path: string, options?: On): { doc: number; pane: string | null } | null;
/** Records which herdr pane shows the document (for focusing it later). */
export function notePane(doc: number, pane: string | null): null;
/** The document a pane was last seen showing (for a pane header). */
export function paneDocument(pane: string): DocumentState | null;
/** How many open documents have unsaved changes. */
export function unsavedCount(): number;
/** Resolves with the new `seq` once the document changed after `seq` (0 once it is gone). */
export function documentChanged(doc: number, seq: number): Promise<number>;
/** Writes it. `force`: even if the file changed on disk since it was read. */
export function saveDocument(doc: number, options?: { force?: boolean }): Promise<{ ok: boolean; conflict: boolean; error: string | null }>;
/** Reads the file again, dropping unsaved changes. */
export function reloadDocument(doc: number): null;
/** Keeps the unsaved changes over a change on disk; the next save overwrites it. */
export function keepMine(doc: number): null;
/** Compares with the disk: a clean document follows a change made elsewhere, a dirty one is marked in conflict. */
export function checkDisk(doc: number): Promise<null>;
export function focusDocument(doc: number): null;
/** Puts the cursor on a 1-based line (and column) and focuses the editor. */
export function gotoLine(doc: number, line: number, column?: number): null;
/** Highlights matches and moves to the next one; empty query ends the search. Counts are in documentState. */
export function find(doc: number, query: string, options?: { caseSensitive?: boolean }): null;
export function findStep(doc: number, backwards?: boolean): null;
export function endFind(doc: number): null;
export function replaceOne(doc: number, replacement: string): Promise<null>;
export function replaceAll(doc: number, replacement: string): Promise<null>;

/** The code editor for a document. One pane at a time edits it (`owner`, any stable string per pane copy);
 *  others show "edit here". Give it a sized parent. `status`: the Ln/Col bar (default true); `theme`: its right-hand label. */
export const Editor: { new: (id: string, props: EditorLook & { doc: number; owner: string; status?: boolean; theme?: string }) => import("gpui-kit").NativeElement };
/** A document rendered as Markdown (GFM), live with its editor. Relative images load from its folder. */
export const Markdown: { new: (id: string, props: { doc: number; colors?: { foreground?: string; body?: string; muted?: string; link?: string; code?: string; border?: string } }) => import("gpui-kit").NativeElement };
/** An image (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, SVG). Bump `rev` to show a new version of the file. */
export const ImageView: { new: (id: string, props: { path: string; on?: string | null; rev?: number; fit?: "contain" | "actual"; colors?: { muted?: string; checker?: string } }) => import("gpui-kit").NativeElement };
"##;

static EN: LazyLock<Catalog> =
    LazyLock::new(|| Catalog::from_json(include_str!("../i18n/en.json"), None));
static ZH_TW: LazyLock<Catalog> = LazyLock::new(|| {
    Catalog::from_json(
        include_str!("../i18n/en.json"),
        Some(include_str!("../i18n/zh-TW.json")),
    )
});

/// This crate's own strings in the shown language.
pub(crate) fn strings() -> &'static Catalog {
    if yuzora_core::i18n::current() == yuzora_core::i18n::ZH_TW {
        &ZH_TW
    } else {
        &EN
    }
}

fn on(a: &HostArguments, i: usize) -> Option<String> {
    opt_str(a, i, "on").filter(|m| !m.is_empty())
}

fn opt_bool(a: &HostArguments, i: usize, key: &str) -> bool {
    a.get(i)
        .and_then(|o| o.get(key))
        .and_then(HostValue::as_bool)
        .unwrap_or(false)
}

fn opt_usize(a: &HostArguments, i: usize, key: &str, default: usize, max: usize) -> usize {
    a.get(i)
        .and_then(|o| o.get(key))
        .and_then(HostValue::as_number)
        .map(|n| (n.max(1.0) as usize).min(max))
        .unwrap_or(default)
}

fn json(v: impl serde::Serialize) -> HostValue {
    serde_json::to_value(v)
        .map(|v| to_host(&v))
        .unwrap_or(HostValue::Null)
}

fn language(l: Lang) -> HostValue {
    HostObject::new()
        .field("id", l.id())
        .field("name", l.name())
        .field("badge", l.badge())
        .field("color", format!("#{:06x}", l.badge_color()))
        .into()
}

/// Which pane a file opens in.
pub fn kind_of(path: &str) -> &'static str {
    if image::format_of(path).is_some() {
        "image"
    } else if Lang::detect(path) == Some(Lang::Markdown) {
        "markdown"
    } else {
        "text"
    }
}

fn app<R>(f: impl FnOnce(&mut App) -> R) -> Result<R, HostError> {
    gpui_shell::with_current_app(f).ok_or_else(|| HostError::new("yuzora:files needs the app"))
}

fn doc_id(a: &HostArguments) -> Result<u64, HostError> {
    Ok(a.number(0)?.max(0.0) as u64)
}

/// Runs `f` on a document, if it is open.
fn with_doc<R>(
    a: &HostArguments,
    f: impl FnOnce(&mut docs::DocView, &mut gpui::Context<docs::DocView>) -> R,
) -> Result<Option<R>, HostError> {
    let id = doc_id(a)?;
    app(|cx| docs::get(id, cx).map(|view| view.update(cx, f)))
}

/// (root, machine, hidden) → when it was listed, and the files.
type Listings = HashMap<(String, Option<String>, bool), (Instant, Arc<search::FileList>)>;
static LISTINGS: LazyLock<Mutex<Listings>> = LazyLock::new(Default::default);
const LISTING_TTL: Duration = Duration::from_secs(15);
const LISTING_LIMIT: usize = 200_000;

fn listing(
    root: &str,
    on: Option<&str>,
    hidden: bool,
    fresh: bool,
) -> Result<Arc<search::FileList>, String> {
    let key = (root.to_owned(), on.map(str::to_owned), hidden);
    if !fresh
        && let Some((at, files)) = LISTINGS.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
        && at.elapsed() < LISTING_TTL
    {
        return Ok(files.clone());
    }
    let files = Arc::new(search::list_files(root, on, hidden, LISTING_LIMIT)?);
    let mut cache = LISTINGS.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (at, _)| at.elapsed() < LISTING_TTL);
    cache.insert(key, (Instant::now(), files.clone()));
    Ok(files)
}

/// A blocking job as a script promise.
fn job<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(T) -> HostValue + Send + 'static,
) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send + 'static {
    blocking(work).map(|r| match r {
        Some(Ok(v)) => Ok(done(v)),
        Some(Err(e)) => Err(HostError::new(e)),
        None => Err(HostError::new("could not start a worker")),
    })
}

fn null(_: ()) -> HostValue {
    HostValue::Null
}

/// The `yuzora:files` module.
/// When the app started: the module is made as the app starts.
static STARTED_AT: LazyLock<u64> = LazyLock::new(docs::now_ms);

pub fn host_module() -> HostModule {
    LazyLock::force(&STARTED_AT);
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| {
            Ok(HostValue::Str(yuzora_core::VERSION.into()))
        })
        .function("startedAt", |_| Ok(HostValue::Number(*STARTED_AT as f64)))
        .function("languageFor", |a| {
            Ok(Lang::detect(a.string(0)?)
                .map(language)
                .unwrap_or(HostValue::Null))
        })
        .function("languages", |_| {
            Ok(HostValue::Array(
                Lang::all().iter().copied().map(language).collect(),
            ))
        })
        .function("kindOf", |a| {
            Ok(HostValue::Str(kind_of(a.string(0)?).into()))
        })
        // Files and folders.
        .async_function("listDir", |a| {
            let (path, on, limit) = (
                a.string(0)?.to_owned(),
                on(a, 1),
                opt_usize(a, 1, "limit", 2000, 20_000),
            );
            Ok(job(
                move || fsops::list_dir(&path, on.as_deref(), limit),
                |(entries, truncated)| {
                    HostObject::new()
                        .field("entries", json(entries))
                        .field("truncated", truncated)
                        .into()
                },
            ))
        })
        .async_function("gitStatus", |a| {
            let (cwd, on) = (a.string(0)?.to_owned(), on(a, 1));
            Ok(job(
                move || git::status(&cwd, on.as_deref(), 20_000),
                |status| match status {
                    None => HostValue::Null,
                    Some(s) => HostObject::new()
                        .field("top", s.top)
                        .field("branch", s.branch)
                        .field("detached", s.detached)
                        .field("ahead", s.ahead)
                        .field("behind", s.behind)
                        .field(
                            "marks",
                            HostValue::Object(
                                s.marks
                                    .into_iter()
                                    .map(|(p, m)| (p, HostValue::Str(m.to_string())))
                                    .collect(),
                            ),
                        )
                        .field("truncated", s.truncated)
                        .into(),
                },
            ))
        })
        .async_function("stat", |a| {
            let (path, on) = (a.string(0)?.to_owned(), on(a, 1));
            Ok(job(
                move || fsops::stat(&path, on.as_deref()),
                |s| match s {
                    None => HostValue::Null,
                    Some(s) => HostObject::new()
                        .field("len", s.len)
                        .field("mtime_ms", s.mtime_ms)
                        .into(),
                },
            ))
        })
        .async_function("createFile", |a| {
            let (path, on) = (a.string(0)?.to_owned(), on(a, 1));
            Ok(job(move || fsops::create_file(&path, on.as_deref()), null))
        })
        .async_function("createFolder", |a| {
            let (path, on) = (a.string(0)?.to_owned(), on(a, 1));
            Ok(job(move || fsops::create_dir(&path, on.as_deref()), null))
        })
        .async_function("rename", |a| {
            let (from, to, on) = (a.string(0)?.to_owned(), a.string(1)?.to_owned(), on(a, 2));
            Ok(job(move || fsops::rename(&from, &to, on.as_deref()), null))
        })
        .async_function("trash", |a| {
            let path = a.string(0)?.to_owned();
            Ok(job(move || fsops::trash(&path), null))
        })
        .function("reveal", |a| {
            fsops::reveal(a.string(0)?).map_err(error)?;
            Ok(HostValue::Null)
        })
        .function("copyText", |a| {
            let text = a.string(0)?.to_owned();
            app(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)))?;
            Ok(HostValue::Null)
        })
        // Search.
        .async_function("findFiles", |a| {
            let (root, query, on) = (a.string(0)?.to_owned(), a.string(1)?.to_owned(), on(a, 2));
            let (hidden, fresh, limit) = (
                opt_bool(a, 2, "hidden"),
                opt_bool(a, 2, "fresh"),
                opt_usize(a, 2, "limit", 100, 2000),
            );
            Ok(job(
                move || {
                    let list = listing(&root, on.as_deref(), hidden, fresh)?;
                    Ok(serde_json::json!({
                        "matches": search::fuzzy_filter(&query, &list.files, limit),
                        "truncated": list.truncated,
                        "unfiltered": list.unfiltered,
                    }))
                },
                json,
            ))
        })
        .async_function("grep", |a| {
            let (root, query, on) = (a.string(0)?.to_owned(), a.string(1)?.to_owned(), on(a, 2));
            let (regex, case, hidden) = (
                opt_bool(a, 2, "regex"),
                opt_bool(a, 2, "caseSensitive"),
                opt_bool(a, 2, "hidden"),
            );
            let limit = opt_usize(a, 2, "limit", 500, 5000);
            Ok(job(
                move || {
                    if search::ripgrep_available(on.as_deref()) {
                        search::grep(&root, on.as_deref(), &query, regex, case, hidden, limit)
                            .map(Some)
                    } else if on.is_none() {
                        search::grep_local(&root, &query, regex, case, hidden, limit).map(Some)
                    } else {
                        Ok(None)
                    }
                },
                |r| match r {
                    None => HostObject::new()
                        .field("available", false)
                        .field("hits", HostValue::Array(Vec::new()))
                        .field("truncated", false)
                        .field("files", 0)
                        .into(),
                    Some(r) => HostObject::new()
                        .field("available", true)
                        .field("hits", json(&r.hits))
                        .field("truncated", r.truncated)
                        .field("files", r.files)
                        .into(),
                },
            ))
        })
        // Documents.
        .function("openDocument", |a| {
            let key = DocKey {
                on: on(a, 1),
                path: a.string(0)?.to_owned(),
            };
            Ok(HostValue::from(app(|cx| docs::open(key, cx))?))
        })
        .function("releaseDocument", |a| {
            let id = doc_id(a)?;
            let owner = a.get(1).and_then(HostValue::as_str).map(str::to_owned);
            app(|cx| docs::release(id, owner.as_deref(), cx))?;
            Ok(HostValue::Null)
        })
        .function("documentState", |a| {
            let id = doc_id(a)?;
            app(|cx| {
                docs::get(id, cx)
                    .map(|v| v.read(cx).snapshot(cx))
                    .unwrap_or(HostValue::Null)
            })
        })
        .function("documentFor", |a| {
            let key = DocKey {
                on: on(a, 1),
                path: a.string(0)?.to_owned(),
            };
            app(|cx| {
                let Some(id) = docs::find(&key, cx) else {
                    return HostValue::Null;
                };
                let pane = docs::get(id, cx).and_then(|v| v.read(cx).pane().map(str::to_owned));
                HostObject::new()
                    .field("doc", id)
                    .field("pane", pane)
                    .into()
            })
        })
        .function("notePane", |a| {
            let pane = a.get(1).and_then(HostValue::as_str).map(str::to_owned);
            with_doc(a, |d, _| d.set_pane(pane))?;
            Ok(HostValue::Null)
        })
        .function("paneDocument", |a| {
            let pane = a.string(0)?.to_owned();
            app(|cx| {
                docs::for_pane(&pane, cx)
                    .map(|v| v.read(cx).snapshot(cx))
                    .unwrap_or(HostValue::Null)
            })
        })
        .function("unsavedCount", |_| {
            Ok(HostValue::from(app(|cx| docs::unsaved(cx))?))
        })
        .async_function("documentChanged", |a| {
            let seen = a.number(1)?.max(0.0) as u64;
            let rx = with_doc(a, |d, _| d.wait(seen))?;
            Ok(async move {
                match rx {
                    Some(rx) => Ok(HostValue::from(rx.await.unwrap_or(0))),
                    None => Ok(HostValue::from(0)),
                }
            })
        })
        .async_function("saveDocument", |a| {
            let force = opt_bool(a, 1, "force");
            let rx = with_doc(a, |d, cx| d.save(force, cx))?
                .ok_or_else(|| HostError::new("no such document"))?;
            Ok(async move {
                Ok(rx
                    .await
                    .unwrap_or_else(|_| docs::Saved {
                        ok: false,
                        conflict: false,
                        error: Some("closed".into()),
                    })
                    .to_host())
            })
        })
        .function("reloadDocument", |a| {
            with_doc(a, |d, cx| d.reload(cx))?;
            Ok(HostValue::Null)
        })
        .function("keepMine", |a| {
            with_doc(a, |d, cx| d.keep_mine(cx))?;
            Ok(HostValue::Null)
        })
        .async_function("checkDisk", |a| {
            let rx = with_doc(a, |d, cx| d.check_disk(cx))?;
            Ok(async move {
                if let Some(rx) = rx {
                    let _ = rx.await;
                }
                Ok(HostValue::Null)
            })
        })
        .function("focusDocument", |a| {
            with_doc(a, |d, cx| d.focus(cx))?;
            Ok(HostValue::Null)
        })
        .function("gotoLine", |a| {
            let line = a.number(1)?.max(1.0) as u32;
            let column = a
                .get(2)
                .and_then(HostValue::as_number)
                .unwrap_or(1.0)
                .max(1.0) as u32;
            with_doc(a, |d, cx| d.goto(line, column, cx))?;
            Ok(HostValue::Null)
        })
        .function("find", |a| {
            let query = a.string(1)?.to_owned();
            let case = opt_bool(a, 2, "caseSensitive");
            with_doc(a, |d, cx| d.find(&query, case, cx))?;
            Ok(HostValue::Null)
        })
        .function("findStep", |a| {
            let backwards = a.get(1).and_then(HostValue::as_bool).unwrap_or(false);
            with_doc(a, |d, cx| d.find_step(backwards, cx))?;
            Ok(HostValue::Null)
        })
        .function("endFind", |a| {
            with_doc(a, |d, cx| d.end_find(cx))?;
            Ok(HostValue::Null)
        })
        .async_function("replaceOne", |a| {
            let replacement = a.string(1)?.to_owned();
            let rx = with_doc(a, |d, cx| d.replace_one(replacement, cx))?;
            Ok(async move {
                if let Some(rx) = rx {
                    let _ = rx.await;
                }
                Ok(HostValue::Null)
            })
        })
        .async_function("replaceAll", |a| {
            let replacement = a.string(1)?.to_owned();
            let rx = with_doc(a, |d, cx| d.replace_all(replacement, cx))?;
            Ok(async move {
                if let Some(rx) = rx {
                    let _ = rx.await;
                }
                Ok(HostValue::Null)
            })
        })
        // Components.
        .component("Editor", docs::editor_component)
        .component("Markdown", markdown::markdown_component)
        .component("ImageView", image::image_component)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:files");
        assert_eq!(
            m.call("version", &HostArguments::new([])).unwrap(),
            HostValue::Str(yuzora_core::VERSION.into())
        );
    }

    #[test]
    fn files_open_in_the_pane_for_their_kind() {
        assert_eq!(kind_of("a/b.png"), "image");
        assert_eq!(kind_of("logo.SVG"), "image");
        assert_eq!(kind_of("README.md"), "markdown");
        assert_eq!(kind_of("main.rs"), "text");
        assert_eq!(kind_of("Makefile"), "text");
        let m = host_module();
        let rust = m
            .call(
                "languageFor",
                &HostArguments::new([HostValue::from("src/lib.rs")]),
            )
            .unwrap();
        assert_eq!(rust.get("id"), Some(&HostValue::from("rust")));
        assert_eq!(
            m.call(
                "languageFor",
                &HostArguments::new([HostValue::from("x.unknown")])
            )
            .unwrap(),
            HostValue::Null
        );
    }

    #[test]
    fn the_editor_and_markdown_panes_share_one_implementation() {
        assert_eq!(
            include_str!("../../../defaults/mods/yuzora-editor/doc-pane.js"),
            include_str!("../../../defaults/mods/yuzora-markdown/doc-pane.js"),
            "copy defaults/mods/yuzora-editor/doc-pane.js to yuzora-markdown"
        );
    }

    #[test]
    fn every_l5_mod_names_each_string_in_both_languages() {
        for (name, en, zh) in [
            (
                "yuzora-sidebar-files",
                include_str!("../../../defaults/mods/yuzora-sidebar-files/i18n/en.json"),
                include_str!("../../../defaults/mods/yuzora-sidebar-files/i18n/zh-TW.json"),
            ),
            (
                "yuzora-editor",
                include_str!("../../../defaults/mods/yuzora-editor/i18n/en.json"),
                include_str!("../../../defaults/mods/yuzora-editor/i18n/zh-TW.json"),
            ),
            (
                "yuzora-markdown",
                include_str!("../../../defaults/mods/yuzora-markdown/i18n/en.json"),
                include_str!("../../../defaults/mods/yuzora-markdown/i18n/zh-TW.json"),
            ),
            (
                "yuzora-files-image",
                include_str!("../../../defaults/mods/yuzora-files-image/i18n/en.json"),
                include_str!("../../../defaults/mods/yuzora-files-image/i18n/zh-TW.json"),
            ),
        ] {
            assert_eq!(
                key_set(en),
                key_set(zh),
                "{name}: en and zh-TW name different strings"
            );
        }
    }

    fn key_set(text: &str) -> Vec<String> {
        fn keys(v: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
            if let Some(m) = v.as_object() {
                for (k, v) in m {
                    keys(v, &format!("{prefix}{k}."), out);
                }
            } else {
                out.push(prefix.trim_end_matches('.').to_owned());
            }
        }
        let mut out = Vec::new();
        keys(&serde_json::from_str(text).unwrap(), "", &mut out);
        out.sort();
        out
    }

    #[test]
    fn both_catalogs_name_every_string() {
        let en: serde_json::Value = serde_json::from_str(include_str!("../i18n/en.json")).unwrap();
        let zh: serde_json::Value =
            serde_json::from_str(include_str!("../i18n/zh-TW.json")).unwrap();
        fn keys(v: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
            if let Some(m) = v.as_object() {
                for (k, v) in m {
                    keys(v, &format!("{prefix}{k}."), out);
                }
            } else {
                out.push(prefix.trim_end_matches('.').to_owned());
            }
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        keys(&en, "", &mut a);
        keys(&zh, "", &mut b);
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }
}
