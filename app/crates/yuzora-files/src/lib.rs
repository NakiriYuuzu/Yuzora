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
mod remote_entry;
pub mod remote_write;
pub mod search;
pub mod svg;

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
/** `color`: a theme color's name (`theme()[color]`). */
export interface Language { id: string; name: string; badge: string; color: string }
export interface GitMarks { top: string; branch: string | null; detached: boolean; ahead: number; behind: number;
  /** Absolute path → "M" | "A" | "D" | "U" | "?". */
  marks: Record<string, string>; truncated: boolean;
  /** The folder waits for trust (its own config names programs git would run): only the branch, no marks. */
  untrusted: boolean }
export interface GrepHit { path: string; line: number; column: number; text: string; ranges: [number, number][] }
export interface DocumentState {
  doc: number; seq: number; path: string; on: string | null; name: string; dir: string;
  phase: "loading" | "ready" | "failed"; error: string | null;
  /** `badgeColor`: a theme color's name (`theme()[badgeColor]`). */
  language: string | null; languageName: string | null; badge: string | null; badgeColor: string | null;
  eol: "LF" | "CRLF"; bom: boolean; size: number;
  binary: boolean; tooLarge: boolean; large: boolean; notUtf8: boolean; readonly: boolean; remote: boolean;
  dirty: boolean; saving: boolean; conflict: boolean; deleted: boolean;
  /** When someone else last changed the file on disk (ms since the epoch). */
  externalAt: number | null;
  focused: boolean; owner: string | null; pane: string | null;
  line: number; column: number; matches: number; match: number | null;
  /** The last line asked to be shown from outside its panes (revealLine): a preview switches to the source for it. */
  reveal: { id: number; line: number; column: number } | null;
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

/** A folder's entries, folders first. Skips .git and .DS_Store. `skipped`: some entries are
 *  left out unread (a name that is not UTF-8, a kind that can't be read, a read that failed). */
export function listDir(path: string, options?: On & { limit?: number }): Promise<{ entries: FileEntry[]; truncated: boolean; skipped: boolean }>;
/** git status for the repo `cwd` is in; null outside a repo. Writes nothing under .git. */
export function gitStatus(cwd: string, options?: On): Promise<GitMarks | null>;
export function stat(path: string, options?: On): Promise<{ len: number; mtime_ms: number } | null>;
/** New file, new folder, rename: here, or on the machine in `on`. Nothing already at the name is replaced. */
export function createFile(path: string, options?: On): Promise<null>;
export function createFolder(path: string, options?: On): Promise<null>;
/** Renaming also moves the documents open at that path (or under that folder) to the new one. */
export function rename(from: string, to: string, options?: On): Promise<null>;
/** Moves a file or folder to the system's trash (this machine only). */
export function trash(path: string): Promise<null>;
/** Shows it in Finder / the file manager / Explorer (this machine only). */
export function reveal(path: string): null;
export function copyText(text: string): null;
/** Fuzzy file-name search under root (.gitignore honored); the listing is cached for 15 s.
 *  `truncated`: only the first 200,000 files were searched. `unfiltered`: the machine has
 *  neither git nor ripgrep, so ignored files are included. `skipped`: a folder could not be read. */
export function findFiles(root: string, query: string, options?: On & { hidden?: boolean; limit?: number; fresh?: boolean }): Promise<{ matches: { path: string; score: number; indices: number[] }[];
  /** More files matched than `limit` (default 100): only the best are in `matches`. */
  more: boolean; truncated: boolean; unfiltered: boolean; skipped: boolean; tooLong: boolean }>;
/** Content search: ripgrep where it is installed, else a built-in search on this machine; `available` is false on another machine without rg.
 *  `skipped`: a file or folder could not be read, so hits may be missing. */
export function grep(root: string, query: string, options?: On & { regex?: boolean; caseSensitive?: boolean; hidden?: boolean; limit?: number }): Promise<{ available: boolean; hits: GrepHit[]; truncated: boolean; files: number; skipped: boolean }>;

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
/** gotoLine asked from outside the document's panes (a search hit): a pane showing the Markdown preview switches to the source and goes there. */
export function revealLine(doc: number, line: number, column?: number): null;
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
    LazyLock::new(|| Catalog::from_json(include_str!("../i18n/en.json")));

/// This crate's own strings.
pub(crate) fn strings() -> &'static Catalog {
    &EN
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
        .field("color", l.badge_color())
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

/// (root, machine, hidden) → when its listing began, and the files.
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
    let began = Instant::now();
    let files = Arc::new(search::list_files(root, on, hidden, LISTING_LIMIT)?);
    let mut cache = LISTINGS.lock().unwrap_or_else(|e| e.into_inner());
    Ok(keep_newest(&mut cache, key, began, files))
}

/// Caches `files`, listed from `began`, unless a listing that began later is
/// there already (an older walk that finished last): that one is returned.
fn keep_newest(
    cache: &mut Listings,
    key: (String, Option<String>, bool),
    began: Instant,
    files: Arc<search::FileList>,
) -> Arc<search::FileList> {
    cache.retain(|_, (at, _)| at.elapsed() < LISTING_TTL);
    match cache.get(&key) {
        Some((at, newer)) if *at > began => newer.clone(),
        _ => {
            cache.insert(key, (began, files.clone()));
            files
        }
    }
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
                |list| {
                    HostObject::new()
                        .field("entries", json(list.entries))
                        .field("truncated", list.truncated)
                        .field("skipped", list.skipped)
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
                        .field("untrusted", s.untrusted)
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
            let rx = app(|cx| docs::rename(from, to, on, cx))?;
            Ok(async move {
                match rx.await {
                    Ok(Ok(())) => Ok(HostValue::Null),
                    Ok(Err(e)) => Err(HostError::new(e)),
                    Err(_) => Err(HostError::new("closed")),
                }
            })
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
                    if search::query_too_long(&query) {
                        return Ok(serde_json::json!({
                            "matches": [], "more": false, "truncated": false,
                            "unfiltered": false, "skipped": false, "tooLong": true,
                        }));
                    }
                    let list = listing(&root, on.as_deref(), hidden, fresh)?;
                    let (matches, more) = search::fuzzy_top(&query, &list.files, limit);
                    Ok(serde_json::json!({
                        "matches": matches,
                        "more": more,
                        "truncated": list.truncated,
                        "unfiltered": list.unfiltered,
                        "skipped": list.skipped,
                        "tooLong": false,
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
                        .field("skipped", false)
                        .into(),
                    Some(r) => HostObject::new()
                        .field("available", true)
                        .field("hits", json(&r.hits))
                        .field("truncated", r.truncated)
                        .field("files", r.files)
                        .field("skipped", r.skipped)
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
        .function("revealLine", |a| {
            let line = a.number(1)?.max(1.0) as u32;
            let column = a
                .get(2)
                .and_then(HostValue::as_number)
                .unwrap_or(1.0)
                .max(1.0) as u32;
            with_doc(a, |d, cx| d.reveal(line, column, cx))?;
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
mod tree_state_tests;

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
    fn an_older_listing_never_replaces_a_newer_one() {
        let mut cache = Listings::default();
        let key = || ("/r".to_owned(), None, false);
        let list = |name: &str| {
            Arc::new(search::FileList {
                files: vec![name.to_owned()],
                ..Default::default()
            })
        };
        let (old, new) = (Instant::now(), Instant::now() + Duration::from_millis(5));
        // The fresh walk finishes first, the older one after it.
        assert_eq!(
            keep_newest(&mut cache, key(), new, list("fresh")).files,
            ["fresh"]
        );
        assert_eq!(
            keep_newest(&mut cache, key(), old, list("stale")).files,
            ["fresh"]
        );
        assert_eq!(cache[&key()].1.files, ["fresh"]);
        // A later one replaces it.
        let later = new + Duration::from_millis(5);
        assert_eq!(
            keep_newest(&mut cache, key(), later, list("later")).files,
            ["later"]
        );
    }

    #[test]
    fn sidebar_paths_take_either_slash_in_windows_paths_only() {
        let src = include_str!("../../../defaults/mods/yuzora-sidebar-files/paths.js")
            .replace("export const", "const");
        let cases = [
            (r#"parent("C:\\a\\b.txt")"#, r"C:\a"),
            (r#"base("C:\\a\\b.txt")"#, "b.txt"),
            (r#"parent("C:\\a.txt")"#, r"C:\"),
            (r#"parent("C:/a/b\\c.txt")"#, "C:/a/b"),
            (r#"base("\\\\srv\\share\\x.md")"#, "x.md"),
            (r#"parent("/a/b")"#, "/a"),
            (r#"parent("/a")"#, "/"),
            (r#"parent("a")"#, ""),
            (r#"base("/a/b")"#, "b"),
            // Elsewhere a backslash is part of a name.
            (r#"base("/a/b\\c")"#, r"b\c"),
            (r#"parent("/a/b\\c")"#, "/a"),
        ];
        let calls: Vec<&str> = cases.iter().map(|(call, _)| *call).collect();
        let script = format!("(() => {{ {src}\nreturn [{}]; }})()", calls.join(", "));
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let got: Vec<String> = context.with(|ctx| ctx.eval(script).unwrap());
        let want: Vec<&str> = cases.iter().map(|(_, want)| *want).collect();
        assert_eq!(got, want);
    }

    /// `readGit` is the only thing between `gitStatus` and the tree: "not a
    /// repo" (null) and "could not be read" (a rejection) must stay apart.
    #[test]
    fn sidebar_git_state_tells_a_failure_from_not_a_repo() {
        let src = include_str!("../../../defaults/mods/yuzora-sidebar-files/git-state.js")
            .replace("export async function", "async function");
        let script = format!(
            r#"(async () => {{ {src}
const a = await readGit(async () => null, "/x", {{}});
const b = await readGit(async () => ({{ top: "/r" }}), "/r", {{}});
const c = await readGit(async () => {{ throw new Error("timed out"); }}, "/r", {{}});
const d = await readGit(() => {{ throw "boom"; }}, "/r", {{}});
return JSON.stringify([a, b, c, d]); }})()"#
        );
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let got: String = context.with(|ctx| {
            let promise: quickjs::Promise = ctx.eval(script).unwrap();
            promise.finish().unwrap()
        });
        assert_eq!(
            got,
            r#"[{"git":null,"failed":null},{"git":{"top":"/r"},"failed":null},{"git":null,"failed":"timed out"},{"git":null,"failed":"boom"}]"#
        );
    }

    /// `failure.js` reads the sentences made here (fsops.rs, remote_entry.rs), so it is run on what they make:
    /// a sentence changed on this side without it changes what the toast says first.
    #[test]
    fn sidebar_failures_are_told_by_their_cause_from_the_messages_made_here() {
        use crate::remote_entry::{Act, Kind, Problem, interpret};
        use yuzora_core::remote::ExecError;
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str| format!("{}/{name}", dir.path().display());
        std::fs::write(at("taken.txt"), "").unwrap();
        std::fs::write(at("a.txt"), "").unwrap();
        let failed = |r: Result<(), String>| r.unwrap_err();
        let mut cases: Vec<(String, &str, &str, &str)> = vec![
            (
                failed(fsops::create_file(&at("taken.txt"), None)),
                "taken.txt",
                "new-file",
                "taken.txt already exists",
            ),
            (
                failed(fsops::create_dir(&at("taken.txt"), None)),
                "taken.txt",
                "new-folder",
                "taken.txt already exists",
            ),
            (
                failed(fsops::create_file(&at("gone/n.txt"), None)),
                "n.txt",
                "new-file",
                "The folder no longer exists",
            ),
            (
                failed(fsops::rename(&at("a.txt"), &at("taken.txt"), None)),
                "taken.txt",
                "rename",
                "taken.txt already exists",
            ),
            (
                failed(fsops::rename(&at("none.txt"), &at("b.txt"), None)),
                "b.txt",
                "rename",
                "That item is no longer there",
            ),
        ];
        #[cfg(unix)]
        // SAFETY: a plain syscall with no arguments.
        if unsafe { libc::geteuid() } != 0 {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::create_dir(at("ro")).unwrap();
            std::fs::set_permissions(at("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
            cases.push((
                failed(fsops::create_file(&at("ro/n.txt"), None)),
                "n.txt",
                "new-file",
                "The folder is not writable",
            ));
            std::fs::set_permissions(at("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let timed = interpret(Err(ExecError::TimedOut {
            seconds: 30.0,
            machine: Some("lab".into()),
        }))
        .unwrap_err();
        let offline = interpret(Err(ExecError::Remote(
            "machine lab is not connected".into(),
        )))
        .unwrap_err();
        let make = Act::Make("/srv/n.txt", Kind::File);
        let rename = Act::Move("/srv/a.txt", "/srv/b.txt");
        for (act, problem, name, kind, want) in [
            (
                &make,
                Problem::Exists,
                "n.txt",
                "new-file",
                "n.txt already exists",
            ),
            (
                &rename,
                Problem::Gone,
                "b.txt",
                "rename",
                "That item is no longer there",
            ),
            (
                &make,
                Problem::NoFolder,
                "n.txt",
                "new-file",
                "The folder no longer exists",
            ),
            (
                &make,
                Problem::FolderNotWritable,
                "n.txt",
                "new-file",
                "The folder is not writable",
            ),
            (
                &rename,
                Problem::OtherDevice,
                "b.txt",
                "rename",
                "A rename can't copy to another disk",
            ),
            (
                &rename,
                Problem::NoTool("mv"),
                "b.txt",
                "rename",
                "lab has no mv",
            ),
            (
                &make,
                Problem::NoShell,
                "n.txt",
                "new-file",
                "Windows machines can't be changed here",
            ),
            (
                &make,
                Problem::BadPath,
                "n.txt",
                "new-file",
                "A machine can't use that path",
            ),
            (&make, offline, "n.txt", "new-file", "Can't reach lab"),
            (
                &rename,
                timed,
                "b.txt",
                "rename",
                "lab took too long: check again",
            ),
            (
                &make,
                Problem::Failed(
                    "the answer could not be read; it may or may not have been done".into(),
                ),
                "n.txt",
                "new-file",
                "Not sure it was done: check again",
            ),
        ] {
            cases.push((act.describe(&problem, "lab"), name, kind, want));
        }
        let input: Vec<serde_json::Value> = cases
            .iter()
            .map(|(message, name, kind, _)| {
                serde_json::json!([format!("`yuzora:files.call`: {message}"), name, kind])
            })
            .collect();
        let src = include_str!("../../../defaults/mods/yuzora-sidebar-files/failure.js")
            .replace("export const", "const")
            .replace("export function", "function")
            .replace(r#"import { t } from "yuzora:core";"#, "");
        let script = format!(
            r#"(() => {{
    const en = {strings};
    const t = (key, vars = {{}}) => key.split(".").reduce((o, k) => o?.[k], en).replace(/\{{(\w+)\}}/g, (_, k) => vars[k]);
    {src}
    return JSON.stringify({input}.map(([message, name, kind]) => reason(new Error(message), {{ name, machine: "lab", kind }})));
    }})()"#,
            strings = include_str!("../../../defaults/mods/yuzora-sidebar-files/i18n/en.json"),
            input = serde_json::Value::Array(input),
        );
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let got: String = context.with(|ctx| ctx.eval(script).unwrap());
        let got: Vec<String> = serde_json::from_str(&got).unwrap();
        let want: Vec<&str> = cases.iter().map(|c| c.3).collect();
        assert_eq!(
            got,
            want,
            "{:#?}",
            cases.iter().map(|c| &c.0).collect::<Vec<_>>()
        );
    }

    /// What a unit test of `readGit` cannot see: the sidebar uses it, shows
    /// its failure and the too-long query note, and does not swallow errors.
    #[test]
    fn the_files_sidebar_is_wired_to_git_failures_and_long_queries() {
        let main = include_str!("../../../defaults/mods/yuzora-sidebar-files/main.js");
        for needle in [
            "readGit: (cwd, options) => readGit(gitStatus, cwd, options),",
            r#"Boolean(note.gitFailed), el => el.child(div().flex_none().px(12).py(6).text_size(11).text_color(th.dim).child(t("git.failed")))"#,
            r#"found.tooLong ? t("search.too_long")"#,
        ] {
            assert_eq!(main.matches(needle).count(), 1, "{needle}");
        }
        let lib = include_str!("lib.rs");
        let code = lib.split("#[cfg(test)]\nmod tests {").next().unwrap();
        assert_eq!(
            code.matches("if search::query_too_long(&query) {").count(),
            1
        );
        assert!(!main.contains("gitStatus(here.cwd, { on: here.on }).catch"));
    }

    #[test]
    fn sidebar_heading_splits_the_folder_name_from_where_it_is() {
        let src = include_str!("../../../defaults/mods/yuzora-sidebar-files/paths.js")
            .replace("export const", "const");
        // (root, this machine's home, branch) → (name, dir ending in its separator, branch).
        let cases = [
            (
                ("/Users/me/App/yuzora", Some("/Users/me"), Some("main")),
                ("yuzora", "~/App/", Some("main")),
            ),
            (
                ("/Users/me/proj", Some("/Users/me"), Some("feat/x")),
                ("proj", "~/", Some("feat/x")),
            ),
            (
                ("/opt/src/tool", Some("/Users/me"), None),
                ("tool", "/opt/src/", None),
            ),
            (("/srv", Some("/Users/me"), None), ("srv", "/", None)),
            // Not in a repo, or no branch name: no branch.
            (
                ("/Users/me/notes", Some("/Users/me"), Some("")),
                ("notes", "~/", None),
            ),
            // The home folder itself, and a root, have nothing above them shown.
            (("/Users/me", Some("/Users/me"), None), ("~", "", None)),
            (("/", Some("/Users/me"), None), ("/", "", None)),
            (
                ("/Users/meow/x", Some("/Users/me"), None),
                ("x", "/Users/meow/", None),
            ),
            // Another machine's path: never "~", whatever this machine's home is.
            (
                ("/home/me/repo", None, Some("dev")),
                ("repo", "/home/me/", Some("dev")),
            ),
            (("/home/me", None, None), ("me", "/home/", None)),
            // Windows, with either slash.
            (
                (r"C:\Users\me\src\app", Some(r"C:\Users\me"), None),
                ("app", r"~\src\", None),
            ),
            (
                (r"C:\app", Some(r"C:\Users\me"), None),
                ("app", r"C:\", None),
            ),
            (("C:/work/app", None, None), ("app", "C:/work/", None)),
            (
                (r"C:\Users\me", Some(r"C:\Users\me"), None),
                ("~", "", None),
            ),
            ((r"C:\", Some(r"C:\Users\me"), None), (r"C:\", "", None)),
        ];
        let calls: Vec<String> = cases
            .iter()
            .map(|((root, home, branch), _)| {
                format!(
                    "heading({root:?}, {}, {})",
                    home.map_or("null".into(), |h| format!("{h:?}")),
                    branch.map_or("null".into(), |b| format!("{b:?}"))
                )
            })
            .collect();
        let script = format!(
            "(() => {{ {src}\nreturn JSON.stringify([{}].map(h => [h.name, h.dir, h.branch])); }})()",
            calls.join(", ")
        );
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let out: String = context.with(|ctx| ctx.eval(script).unwrap());
        let got: Vec<(String, String, Option<String>)> = serde_json::from_str(&out).unwrap();
        let want: Vec<(String, String, Option<String>)> = cases
            .iter()
            .map(|(_, (name, dir, branch))| {
                (name.to_string(), dir.to_string(), branch.map(str::to_owned))
            })
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn file_icons_follow_the_type_and_the_theme() {
        let src = include_str!("../../../defaults/mods/yuzora-sidebar-files/icons.js")
            .replace("export function", "function");
        let cases = [
            (("main.rs", false, false), ("code", "ansi3")),
            (("App.TSX", false, false), ("code", "ansi4")),
            (("index.mjs", false, false), ("code", "ansi11")),
            (("tsconfig.json", false, false), ("json", "ansi11")),
            (("README.md", false, false), ("markdown", "muted")),
            (("Cargo.toml", false, false), ("package", "ansi3")),
            (("rustfmt.toml", false, false), ("config", "ansi6")),
            (("package.json", false, false), ("package", "ansi2")),
            (("Cargo.lock", false, false), ("lock", "dim")),
            (("package-lock.json", false, false), ("lock", "dim")),
            (("Dockerfile", false, false), ("container", "ansi4")),
            (("Dockerfile.dev", false, false), ("container", "ansi4")),
            ((".gitignore", false, false), ("git", "ansi1")),
            ((".env.local", false, false), ("config", "ansi6")),
            (("logo.svg", false, false), ("image", "ansi13")),
            (("build.sh", false, false), ("terminal", "ansi2")),
            (("styles.css", false, false), ("code", "ansi5")),
            (("index.html", false, false), ("code", "ansi1")),
            (("LICENSE", false, false), ("file", "dim")),
            ((".hidden", false, false), ("file", "dim")),
            (("a.unknown", false, false), ("file", "dim")),
            // A folder is a folder, whatever its name.
            (("src.rs", true, false), ("folder", "dim")),
            (("src", true, true), ("folder-open", "dim")),
        ];
        let calls: Vec<String> = cases
            .iter()
            .map(|((name, dir, open), _)| {
                format!("(i => i.icon + ' ' + i.color)(fileIcon({name:?}, {dir}, {open}))")
            })
            .collect();
        // Every icon and color it can give, and how long a big folder's
        // worth of rows takes to look up.
        let script = format!(
            "(() => {{ {src}
            const all = [...BY_NAME.values(), ...BY_EXT.values(), FOLDER, FOLDER_OPEN, FILE];
            const started = Date.now();
            for (let i = 0; i < 20000; i++) fileIcon(`file${{i}}.tsx`);
            return JSON.stringify([[{}], all.map(i => i.icon), all.map(i => i.color), Date.now() - started]);
            }})()",
            calls.join(", ")
        );
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let out: String = context.with(|ctx| ctx.eval(script).unwrap());
        let (got, icons, colors, ms): (Vec<String>, Vec<String>, Vec<String>, f64) =
            serde_json::from_str(&out).unwrap();
        let want: Vec<String> = cases.iter().map(|(_, (i, c))| format!("{i} {c}")).collect();
        assert_eq!(got, want);
        // Only colors every theme names.
        let names = ["foreground", "muted", "dim", "accent"];
        for color in &colors {
            let ansi = color
                .strip_prefix("ansi")
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| n < 16);
            assert!(ansi || names.contains(&color.as_str()), "{color}");
        }
        // Each one drawn, in the mode bar's style: a 16x16 mask with 1.4 strokes.
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../defaults/mods/yuzora-sidebar-files/icons/files"
        );
        for icon in &icons {
            let svg = std::fs::read_to_string(format!("{dir}/{icon}.svg")).unwrap();
            assert!(
                svg.contains(r#"viewBox="0 0 16 16""#) && svg.contains(r#"stroke-width="1.4""#),
                "{icon}"
            );
        }
        assert!(ms < 1000.0, "20000 lookups took {ms} ms");
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

    /// A fake Herdr that follows window.rs. `run` only queues a command; `flush()` applies the queue and
    /// publishes what `panes()`, `spaces()` and `focused()` read, as the window does a moment after a mod
    /// asked (a mod pane lists its props, a terminal does not). `tab.new` selects the new tab (after the
    /// selected one) and its Space, a split focuses inside its own tab, `pane.focus` selects the pane's tab and
    /// Space. A mod pane starts (and opens its document) when its tab is first shown, not before; the pane
    /// that has the keys notes itself on its document, as `doc-pane.js` does, and no other does.
    /// `restore(...)` puts a pane there that the layout had before this run: in a tab of its own that has not
    /// been shown, or with `beside` in the tab shown. `rename(from, to)` follows a file renamed, as `docs.rs` does.
    /// A mod pane is listed with `started` (a copy of its mod is running); a command that names a pane that is no
    /// longer there does nothing, as the window answers it with an error and goes on. `tab.select` shows a tab
    /// and its Space.
    /// `strip(space)` is the tab strip with the panes in each tab and `*` on the selected tab, `focus()` the
    /// pane that has the keys.
    const HERDR_WORLD: &str = r##"
        const world = [
          { id: "s1", machine: null, active: true, selected: "t1", tabs: [
            { id: "t1", name: "", focus: "p1", panes: [{ pane: "p1", mod: null }] },
            { id: "t2", name: "build", focus: "p2", panes: [{ pane: "p2", mod: null }] } ] },
          { id: "s2", machine: "lab", active: false, selected: "t3", tabs: [
            { id: "t3", name: "", focus: "p3", panes: [{ pane: "p3", mod: null }] } ] },
          { id: "s3", machine: null, active: false, selected: "t4", tabs: [
            { id: "t4", name: "", focus: "p4", panes: [{ pane: "p4", mod: null }] } ] },
        ];
        const given = {}, queue = [], revealed = [], focusedDocs = [], docs = new Map(), opens = {}, started = new Set();
        let serial = 10, docs_made = 0, snapshot = null, nobody = false;
        const shown = () => world.find(s => s.active);
        const tabShown = () => shown().tabs.find(t => t.id === shown().selected);
        const tabOf = pane => world.flatMap(s => s.tabs).find(t => t.panes.some(p => p.pane === pane));
        const select = tab => {
          const s = world.find(x => x.tabs.includes(tab));
          s.selected = tab.id;
          for (const x of world) x.active = x === s;
        };
        const keyOf = (path, on) => `${on ?? ""}\0${path}`;
        const DOC_PANES = ["yuzora-editor", "yuzora-markdown"];
        function apply(name, a) {
          if (name === "tab.new") {
            const s = a.space ? world.find(x => x.id === a.space) : shown(), pane = `p${++serial}`;
            const tab = { id: `t${serial}`, name: a.name ?? "", focus: pane, panes: [{ pane, mod: a.mod ?? null }] };
            s.tabs.splice(s.tabs.findIndex(t => t.id === s.selected) + 1, 0, tab);
            given[pane] = a.props;
            select(tab);
          } else if (name === "pane.split") {
            const target = a.pane ?? tabShown().focus, tab = tabOf(target), pane = `p${++serial}`;
            if (!tab) return;
            tab.panes.splice(tab.panes.findIndex(p => p.pane === target) + 1, 0, { pane, mod: a.mod ?? null });
            given[pane] = a.props;
            tab.focus = pane;
          } else if (name === "pane.close") {
            const tab = tabOf(a.pane);
            if (!tab) return;
            tab.panes = tab.panes.filter(p => p.pane !== a.pane);
            if (tab.focus === a.pane) tab.focus = tab.panes[0].pane;
          } else if (name === "pane.focus") {
            const tab = tabOf(a.pane);
            if (!tab) return;
            tab.focus = a.pane;
            select(tab);
          } else if (name === "tab.select") {
            const tab = world.flatMap(s => s.tabs).find(t => t.id === a.id);
            if (tab) select(tab);
          } else throw new Error(name);
        }
        function publish() {
          const tab = tabShown();
          for (const p of tab.panes) {
            const at = given[p.pane];
            if (started.has(p.pane)) continue;
            started.add(p.pane);
            if (!DOC_PANES.includes(p.mod) || !at?.path) continue;
            if (!docs.has(keyOf(at.path, at.on))) docs.set(keyOf(at.path, at.on), { id: ++docs_made, pane: null });
            opens[p.pane] = docs.get(keyOf(at.path, at.on));
          }
          const here = tab.panes.find(p => p.pane === tab.focus);
          if (opens[here.pane]) opens[here.pane].pane = here.pane;
          snapshot = {
            spaces: world.map(s => ({ id: s.id, machine: s.machine, active: s.active, tabs: s.tabs.map(t => ({ id: t.id, name: t.name, active: s.selected === t.id })) })),
            panes: world.flatMap(s => s.tabs.flatMap(t => t.panes.map(p => ({ pane: p.pane, mod: p.mod ?? undefined, ...(p.mod ? { props: given[p.pane], started: started.has(p.pane) } : {}), tab: t.id })))),
            focused: { space: shown().id, tab: tab.id, pane: nobody ? null : tab.focus },
          };
        }
        publish();
        const flush = () => { while (queue.length) apply(...queue.shift()); publish(); };
        const run = (name, args) => { queue.push([name, args]); };
        const spaces = () => snapshot.spaces, panes = () => snapshot.panes, focused = () => snapshot.focused;
        const kindOf = p => (/\.png$/.test(p) ? "image" : /\.md$/.test(p) ? "markdown" : "text");
        const documentFor = (path, o) => { const d = docs.get(keyOf(path, o?.on)); return d ? { doc: d.id, pane: d.pane } : null; };
        const paneDocument = pane => { for (const [key, d] of docs) if (d.pane === pane) return { path: key.split("\0")[1] }; return null; };
        const notePane = (doc, pane) => { for (const d of docs.values()) if (d.id === doc) d.pane = pane; };
        const focusDocument = doc => focusedDocs.push(doc);
        const revealLine = (doc, line, column) => revealed.push([line, column]);
        const show = id => { for (const s of world) s.active = s.id === id; publish(); };
        const restore = (space, name, mod, props, { beside = false, pane = null } = {}) => {
          const s = world.find(x => x.id === space), n = ++serial;
          pane ??= `r${n}`;
          given[pane] = props;
          if (beside) tabShown().panes.push({ pane, mod });
          else s.tabs.push({ id: `tr${n}`, name, focus: pane, panes: [{ pane, mod }] });
          publish();
          return pane;
        };
        const rename = (from, to, on = null) => {
          for (const [key, d] of [...docs]) {
            const [machine, path] = key.split("\0");
            if (machine !== (on ?? "") || !(path === from || path.startsWith(from + "/"))) continue;
            docs.delete(key);
            docs.set(keyOf(to + path.slice(from.length), on), d);
          }
        };
        const label = p => (p.mod === null ? "shell" : `${p.mod.replace(/^yuzora-(files-)?/, "")} ${given[p.pane].path}`);
        const strip = id => {
          const s = world.find(x => x.id === id);
          return s.tabs.map(t => `[${t.panes.map(label).join(", ")}]${s.selected === t.id ? "*" : ""}`).join(" ");
        };
        const names = id => world.find(x => x.id === id).tabs.map(t => t.name || "-").join(" ");
        const focus = () => { const t = tabShown(); return label(t.panes.find(p => p.pane === t.focus)); };
        const paneOf = path => panes().find(p => given[p.pane]?.path === path).pane;
    "##;

    /// Runs `body` against open.js as shipped and `HERDR_WORLD`; `body` returns what it saw.
    fn in_the_open(body: &str) -> Vec<String> {
        let paths = include_str!("../../../defaults/mods/yuzora-sidebar-files/paths.js")
            .replace("export const", "const");
        let open = include_str!("../../../defaults/mods/yuzora-sidebar-files/open.js")
            .lines()
            .filter(|l| !l.starts_with("import ") && !l.starts_with("export {"))
            .collect::<Vec<_>>()
            .join("\n")
            .replace("export function", "function");
        let script = ["(() => {", &paths, HERDR_WORLD, &open, body, "})()"].join("\n");
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        context.with(|ctx| match ctx.eval::<Vec<String>, _>(script) {
            Ok(seen) => seen,
            Err(error) => panic!("{error}: {:?}", ctx.catch()),
        })
    }

    #[test]
    fn a_file_opens_in_a_tab_of_its_own_after_the_one_shown() {
        let seen = in_the_open(
            r#"
            openFile("/r/src/a.rs", null);
            flush();
            return [strip("s1"), names("s1"), focus()];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell] [editor /r/src/a.rs]* [shell]",
                "- a.rs build",
                "editor /r/src/a.rs"
            ]
        );
    }

    #[test]
    fn each_kind_of_file_opens_in_its_own_pane_alone_in_the_new_tab() {
        let seen = in_the_open(
            r#"
            const out = [];
            for (const path of ["/r/a.rs", "/r/README.md", "/r/a.png"]) { openFile(path, null); flush(); out.push(focus()); }
            out.push(strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "editor /r/a.rs",
                "markdown /r/README.md",
                "image /r/a.png",
                "[shell] [editor /r/a.rs] [markdown /r/README.md] [image /r/a.png]* [shell]"
            ]
        );
    }

    #[test]
    fn a_file_opened_to_the_side_splits_the_tab_shown() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null, { place: "side" });
            flush();
            return [strip("s1"), names("s1"), focus()];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell, editor /r/a.rs]* [shell]",
                "- build",
                "editor /r/a.rs"
            ]
        );
    }

    #[test]
    fn a_file_already_open_is_shown_where_it_is_and_not_opened_again() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null); flush();
            run("pane.focus", { pane: "p1" }); flush();
            const away = strip("s1");
            openFile("/r/a.rs", null); flush();
            const byClick = strip("s1");
            return [away, byClick, focus()];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell]* [editor /r/a.rs] [shell]",
                "[shell] [editor /r/a.rs]* [shell]",
                "editor /r/a.rs"
            ]
        );
    }

    #[test]
    fn open_to_the_side_splits_the_focused_pane_though_the_file_is_open_in_a_tab() {
        let seen = in_the_open(
            r#"
            const out = [];
            for (const path of ["/r/a.rs", "/r/README.md", "/r/a.png"]) { openFile(path, null); flush(); }
            run("pane.focus", { pane: "p1" }); flush();
            for (const path of ["/r/a.rs", "/r/README.md", "/r/a.png"]) { openFile(path, null, { place: "side" }); flush(); out.push(focus()); }
            out.push(strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "editor /r/a.rs",
                "markdown /r/README.md",
                "image /r/a.png",
                "[shell, editor /r/a.rs, markdown /r/README.md, image /r/a.png]* [editor /r/a.rs] [markdown /r/README.md] [image /r/a.png] [shell]",
            ]
        );
    }

    #[test]
    fn open_to_the_side_splits_though_the_file_waits_in_a_pane_not_started() {
        let seen = in_the_open(
            r#"
            restore("s1", "a.rs", "yuzora-editor", { path: "/r/a.rs", on: null, opened: 1 });
            restore("s1", "", "yuzora-editor", { path: "/r/b.rs", on: null, opened: 1 }, { beside: true });
            openFile("/r/a.rs", null, { place: "side", line: 4 }); flush();
            return [strip("s1"), JSON.stringify(given[focused().pane].line)];
        "#,
        );
        assert_eq!(
            seen,
            ["[shell, editor /r/a.rs, editor /r/b.rs]* [shell] [editor /r/a.rs]", "4"]
        );
    }

    #[test]
    fn open_to_the_side_with_no_pane_in_focus_opens_a_tab_though_the_file_is_open() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null); flush();
            nobody = true; publish();
            openFile("/r/a.rs", null, { place: "side" }); flush();
            return [strip("s1")];
        "#,
        );
        assert_eq!(seen, ["[shell] [editor /r/a.rs] [editor /r/a.rs]* [shell]"]);
    }

    #[test]
    fn a_file_open_beside_a_shell_is_shown_there_by_a_click() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null, { place: "side" }); flush();
            run("pane.focus", { pane: "p1" }); flush();
            openFile("/r/a.rs", null); flush();
            return [strip("s1"), focus()];
        "#,
        );
        assert_eq!(seen, ["[shell, editor /r/a.rs]* [shell]", "editor /r/a.rs"]);
    }

    #[test]
    fn a_line_is_kept_in_a_new_tab_and_shown_in_one_already_open() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null, { line: 12, column: 3 }); flush();
            const props = given[focused().pane];
            run("pane.focus", { pane: "p1" }); flush();
            openFile("/r/a.rs", null, { line: 40 }); flush();
            return [`${props.line}:${props.column}`, typeof props.opened, JSON.stringify(revealed), strip("s1")];
        "#,
        );
        assert_eq!(
            seen,
            [
                "12:3",
                "number",
                "[[40,1]]",
                "[shell] [editor /r/a.rs]* [shell]"
            ]
        );
    }

    #[test]
    fn a_file_of_a_remote_space_opens_in_a_tab_of_that_space() {
        let seen = in_the_open(
            r#"
            show("s2");
            openFile("/srv/a.rs", "lab"); flush();
            const props = given[focused().pane];
            return [strip("s1"), strip("s2"), `${props.on}:${props.path}`, shown().id];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell]* [shell]",
                "[shell] [editor /srv/a.rs]*",
                "lab:/srv/a.rs",
                "s2"
            ]
        );
    }

    #[test]
    fn a_file_open_in_another_space_is_opened_again_in_the_space_shown() {
        let seen = in_the_open(
            r#"
            const out = [];
            for (const path of ["/r/a.rs", "/r/README.md", "/r/a.png"]) { openFile(path, null); flush(); }
            restore("s1", "b.rs", "yuzora-editor", { path: "/r/b.rs", on: null, opened: 1 });
            const before = strip("s1");
            show("s3");
            for (const path of ["/r/a.rs", "/r/README.md", "/r/a.png", "/r/b.rs"]) { openFile(path, null); flush(); out.push(shown().id + " " + focus()); }
            out.push(strip("s3"), strip("s1") === before ? "s1 as it was" : strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "s3 editor /r/a.rs",
                "s3 markdown /r/README.md",
                "s3 image /r/a.png",
                "s3 editor /r/b.rs",
                "[shell] [editor /r/a.rs] [markdown /r/README.md] [image /r/a.png] [editor /r/b.rs]*",
                "s1 as it was",
            ]
        );
    }

    #[test]
    fn a_pane_of_the_space_shown_is_used_though_the_document_was_last_seen_in_another() {
        let seen = in_the_open(
            r#"
            const at = { path: "/r/a.rs", on: null, opened: 1 };
            const other = restore("s1", "", "yuzora-editor", at, { beside: true });
            const waiting = restore("s3", "a.rs", "yuzora-editor", at);
            run("pane.focus", { pane: other }); flush();
            show("s3");
            openFile("/r/a.rs", null, { line: 5 }); flush();
            return [String(panes().some(p => p.pane === waiting)), String(panes().some(p => p.pane === other)), JSON.stringify(revealed), strip("s3"), given[focused().pane].line + ""];
        "#,
        );
        assert_eq!(seen, ["false", "true", "[]", "[shell] [editor /r/a.rs]*", "5"]);
    }

    #[test]
    fn a_pane_not_started_is_shown_without_the_focus_of_a_document_it_does_not_hold() {
        let seen = in_the_open(
            r#"
            const at = { path: "/r/a.rs", on: null, opened: 1 };
            const other = restore("s1", "", "yuzora-editor", at, { beside: true });
            const waiting = restore("s3", "a.rs", "yuzora-editor", at);
            show("s3");
            openFile("/r/a.rs", null); flush();
            return [String(focused().pane === waiting), JSON.stringify(focusedDocs), strip("s3")];
        "#,
        );
        assert_eq!(seen, ["true", "[]", "[shell] [editor /r/a.rs]*"]);
    }

    #[test]
    fn a_rename_reaches_the_panes_of_every_space() {
        let seen = in_the_open(
            r#"
            restore("s1", "a.rs", "yuzora-editor", { path: "/r/a.rs", on: null, opened: 1 });
            restore("s3", "a.png", "yuzora-files-image", { path: "/r/a.png", on: null, opened: 1 });
            show("s3");
            moved("/r", "/x", null); flush();
            return [strip("s1"), strip("s3")];
        "#,
        );
        assert_eq!(seen, ["[shell]* [shell] [editor /x/a.rs]", "[shell]* [image /x/a.png]"]);
    }

    #[test]
    fn a_file_opens_in_the_space_it_was_asked_in_though_another_is_shown_when_the_command_runs() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null);
            show("s2");
            flush();
            return [strip("s1"), strip("s2")];
        "#,
        );
        assert_eq!(seen, ["[shell] [editor /r/a.rs]* [shell]", "[shell]*"]);
    }

    #[test]
    fn a_place_that_is_not_known_opens_nothing() {
        let seen = in_the_open(
            r#"
            let said = "";
            try { openFile("/r/a.rs", null, { place: "elsewhere" }); } catch (e) { said = String(e.message); }
            flush();
            return [said, strip("s1")];
        "#,
        );
        assert_eq!(seen, ["unknown place: elsewhere", "[shell]* [shell]"]);
    }

    #[test]
    fn with_no_pane_focused_the_side_is_a_tab() {
        let seen = in_the_open(
            r#"
            nobody = true; publish();
            openFile("/r/a.rs", null, { place: "side" }); flush();
            return [strip("s1")];
        "#,
        );
        assert_eq!(seen, ["[shell] [editor /r/a.rs]* [shell]"]);
    }

    #[test]
    fn an_image_open_in_a_tab_is_shown_by_a_second_click() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.png", null); flush();
            run("pane.focus", { pane: "p1" }); flush();
            openFile("/r/a.png", null); flush();
            return [strip("s1"), names("s1"), focus()];
        "#,
        );
        assert_eq!(
            seen,
            ["[shell] [image /r/a.png]* [shell]", "- a.png build", "image /r/a.png"]
        );
    }

    #[test]
    fn a_renamed_image_is_shown_in_the_same_place_under_its_new_path() {
        let seen = in_the_open(
            r#"
            const out = [];
            openFile("/r/a.png", null, { place: "side" }); flush();
            run("pane.focus", { pane: "p1" }); flush();
            moved("/r/a.png", "/r/b.png", null); flush();
            out.push(strip("s1") + " " + focus());
            // Opening the new name finds that pane.
            openFile("/r/b.png", null); flush();
            out.push(strip("s1") + " " + focus());
            run("pane.focus", { pane: "p1" }); flush();
            openFile("/r/img/c.png", null, { place: "side" }); flush();
            moved("/r/im", "/r/x", null); moved("/r/img", "/r/pics", null); flush();
            out.push(strip("s1") + " " + focus());
            // A pane closed meanwhile is not opened again.
            run("pane.close", { pane: paneOf("/r/b.png") }); flush();
            moved("/r/b.png", "/r/d.png", null); flush();
            out.push(strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell, image /r/b.png]* [shell] shell",
                "[shell, image /r/b.png]* [shell] image /r/b.png",
                "[shell, image /r/pics/c.png, image /r/b.png]* [shell] image /r/pics/c.png",
                "[shell, image /r/pics/c.png]* [shell]",
            ]
        );
    }

    #[test]
    fn the_keys_stay_with_the_pane_that_had_them_when_two_panes_of_a_folder_are_replaced() {
        let seen = in_the_open(
            r#"
            const out = [];
            openFile("/r/d/a.png", null); flush(); openFile("/r/d/b.png", null, { place: "side" }); flush();
            const had = paneOf("/r/d/a.png");
            run("pane.focus", { pane: had }); flush();
            rename("/r/d", "/r/e"); moved("/r/d", "/r/e", null); flush();
            out.push(strip("s1"), focus());
            // The same with the other pane in focus.
            run("pane.focus", { pane: paneOf("/r/e/b.png") }); flush();
            moved("/r/e", "/r/f", null); flush();
            out.push(strip("s1"), focus());
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell] [image /r/e/a.png, image /r/e/b.png]* [shell]",
                "image /r/e/a.png",
                "[shell] [image /r/f/a.png, image /r/f/b.png]* [shell]",
                "image /r/f/b.png",
            ]
        );
    }

    #[test]
    fn a_file_whose_pane_was_closed_is_opened_again() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.rs", null, { place: "side" }); openFile("/r/a.png", null, { place: "side" }); flush();
            run("pane.close", { pane: paneOf("/r/a.rs") }); run("pane.close", { pane: paneOf("/r/a.png") }); flush();
            const closed = strip("s1");
            openFile("/r/a.rs", null); openFile("/r/a.png", null); flush();
            return [closed, strip("s1")];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell]* [shell]",
                "[shell] [editor /r/a.rs] [image /r/a.png]* [shell]"
            ]
        );
    }

    #[test]
    fn a_renamed_image_in_a_tab_not_shown_is_found_by_its_own_pane() {
        let seen = in_the_open(
            r#"
            openFile("/r/a.png", null); flush();
            openFile("/r/c.png", null); flush();
            moved("/r/a.png", "/r/b.png", null); flush();
            openFile("/r/b.png", null); flush();
            return [strip("s1"), focus()];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell] [image /r/b.png]* [image /r/c.png] [shell]",
                "image /r/b.png"
            ]
        );
    }

    #[test]
    fn a_file_in_a_tab_the_layout_restored_and_nobody_has_shown_yet_is_shown_not_opened_again() {
        let seen = in_the_open(
            r#"
            const out = [];
            const files = [["yuzora-editor", "/r/a.rs"], ["yuzora-markdown", "/r/README.md"], ["yuzora-files-image", "/r/a.png"]];
            for (const [mod, path] of files) restore("s1", path.split("/").pop(), mod, { path, on: null, opened: 1 });
            const before = strip("s1");
            for (const [, path] of files) { openFile(path, null); flush(); out.push(focus()); }
            out.push(before, strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "editor /r/a.rs",
                "markdown /r/README.md",
                "image /r/a.png",
                "[shell]* [shell] [editor /r/a.rs] [markdown /r/README.md] [image /r/a.png]",
                "[shell] [shell] [editor /r/a.rs] [markdown /r/README.md] [image /r/a.png]*",
            ]
        );
    }

    #[test]
    fn a_file_in_a_pane_the_layout_restored_beside_the_one_shown_is_shown_not_opened_again() {
        let seen = in_the_open(
            r#"
            const out = [];
            const files = [["yuzora-editor", "/r/a.rs"], ["yuzora-markdown", "/r/README.md"], ["yuzora-files-image", "/r/a.png"]];
            for (const [mod, path] of files) restore("s1", "", mod, { path, on: null, opened: 1 }, { beside: true });
            out.push(String(documentFor("/r/a.rs").pane));
            for (const [, path] of files) { openFile(path, null); flush(); out.push(focus()); }
            out.push(strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "null",
                "editor /r/a.rs",
                "markdown /r/README.md",
                "image /r/a.png",
                "[shell, editor /r/a.rs, markdown /r/README.md, image /r/a.png]* [shell]",
            ]
        );
    }

    #[test]
    fn a_file_of_a_remote_space_restored_with_its_layout_is_shown_not_opened_again() {
        let seen = in_the_open(
            r#"
            const out = [];
            restore("s2", "a.rs", "yuzora-editor", { path: "/srv/a.rs", on: "lab", opened: 1 }, { pane: "lab/pa175" });
            restore("s2", "b.rs", "yuzora-editor", { path: "/srv/b.rs", on: "lab", opened: 1 }, { pane: "lab/pb176" });
            show("s2");
            restore("s2", "", "yuzora-editor", { path: "/srv/c.rs", on: "lab", opened: 1 }, { beside: true, pane: "lab/pc177" });
            for (const path of ["/srv/a.rs", "/srv/c.rs"]) { openFile(path, "lab"); flush(); out.push(focused().pane); }
            out.push(strip("s1"), strip("s2"), shown().id);
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "lab/pa175",
                "lab/pc177",
                "[shell]* [shell]",
                "[shell, editor /srv/c.rs]* [editor /srv/a.rs] [editor /srv/b.rs]",
                "s2",
            ]
        );
    }

    #[test]
    fn a_restored_pane_is_for_its_own_machine_only() {
        let seen = in_the_open(
            r#"
            restore("s1", "a.rs", "yuzora-editor", { path: "/srv/a.rs", on: "lab", opened: 1 });
            openFile("/srv/a.rs", null); flush();
            return [strip("s1")];
        "#,
        );
        assert_eq!(
            seen,
            ["[shell] [editor /srv/a.rs]* [shell] [editor /srv/a.rs]"]
        );
    }

    #[test]
    fn a_pane_of_another_mod_holding_the_same_path_is_not_the_files_pane() {
        let seen = in_the_open(
            r#"
            restore("s1", "", "yuzora-git-diff", { path: "/r/a.rs", on: null, cwd: "/r" }, { beside: true });
            restore("s1", "", "yuzora-editor", { path: "/r/a.png", on: null, opened: 1 }, { beside: true });
            openFile("/r/a.rs", null); flush();
            openFile("/r/a.png", null); flush();
            return [strip("s1")];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell, git-diff /r/a.rs, editor /r/a.png] [editor /r/a.rs] [image /r/a.png]* [shell]"
            ]
        );
    }

    #[test]
    fn a_restored_pane_is_found_under_the_name_its_file_was_renamed_to() {
        let seen = in_the_open(
            r#"
            restore("s1", "", "yuzora-editor", { path: "/r/d/a.rs", on: null, opened: 1 }, { beside: true });
            rename("/r/d/a.rs", "/r/d/b.rs"); moved("/r/d/a.rs", "/r/d/b.rs", null); flush();
            openFile("/r/d/b.rs", null); flush();
            const file = strip("s1");
            rename("/r/d", "/r/e"); moved("/r/d", "/r/e", null); flush();
            openFile("/r/e/b.rs", null); flush();
            const folder = strip("s1");
            // The old name is free again: a file made under it is a new one, not the pane of the renamed.
            openFile("/r/d/a.rs", null); flush();
            return [file, folder, strip("s1")];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell, editor /r/d/a.rs]* [shell]",
                "[shell, editor /r/d/a.rs]* [shell]",
                "[shell, editor /r/d/a.rs] [editor /r/d/a.rs]* [shell]",
            ]
        );
    }

    #[test]
    fn a_pane_not_started_yet_is_replaced_under_the_new_name_when_its_file_is_renamed() {
        let seen = in_the_open(
            r#"
            const out = [];
            restore("s1", "a.rs", "yuzora-editor", { path: "/r/a.rs", on: null, opened: 1, line: 9 });
            restore("s1", "R.md", "yuzora-markdown", { path: "/r/d/R.md", on: null, opened: 1 });
            moved("/r/a.rs", "/r/b.rs", null); moved("/r/d", "/r/e", null); flush();
            out.push(strip("s1"), focus());
            openFile("/r/b.rs", null); flush();
            out.push(strip("s1"));
            openFile("/r/e/R.md", null); flush();
            out.push(strip("s1"));
            // The old name is free: a file made there is a new one.
            openFile("/r/a.rs", null); flush();
            out.push(strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell]* [shell] [editor /r/b.rs] [markdown /r/e/R.md]",
                "shell",
                "[shell] [shell] [editor /r/b.rs]* [markdown /r/e/R.md]",
                "[shell] [shell] [editor /r/b.rs] [markdown /r/e/R.md]*",
                "[shell] [shell] [editor /r/b.rs] [markdown /r/e/R.md] [editor /r/a.rs]*",
            ]
        );
    }

    #[test]
    fn with_two_panes_on_one_file_the_one_its_document_was_last_seen_in_is_shown() {
        let seen = in_the_open(
            r#"
            const out = [];
            const at = { path: "/r/a.rs", on: null, opened: 1 };
            const a = restore("s1", "", "yuzora-editor", at, { beside: true }), b = restore("s1", "", "yuzora-editor", at, { beside: true });
            const is = pane => (pane === a ? "first" : pane === b ? "second" : pane);
            openFile("/r/a.rs", null); flush();
            out.push(is(focused().pane));
            run("pane.focus", { pane: b }); flush();
            run("pane.focus", { pane: "p1" }); flush();
            openFile("/r/a.rs", null); flush();
            out.push(is(focused().pane), strip("s1"));
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "first",
                "second",
                "[shell, editor /r/a.rs, editor /r/a.rs]* [shell]"
            ]
        );
    }

    // Two panes on one file: the one beside the shell started with the app, the other waits in a tab not shown yet.
    // `dormant_first` puts the waiting pane ahead of the started one in the list of panes.
    const TWO_PANES_ON_A_FILE: &str = r#"
        const two = (dormantFirst, at) => {
          if (!dormantFirst) {
            const running = restore("s1", "", "yuzora-editor", at, { beside: true });
            return { running, waiting: restore("s1", "a.rs", "yuzora-editor", at) };
          }
          const waiting = restore("s1", "a.rs", "yuzora-editor", at);
          restore("s1", "x.rs", "yuzora-editor", { path: "/r/x.rs", on: null, opened: 1 });
          select(world[0].tabs[3]); publish();
          return { running: restore("s1", "", "yuzora-editor", at, { beside: true }), waiting };
        };
    "#;

    #[test]
    fn a_pane_not_started_is_replaced_when_its_file_is_renamed_though_another_pane_holds_the_document() {
        for (dormant_first, strip) in [
            (false, "[shell, editor /r/a.rs]* [shell] [editor /r/b.rs]"),
            (true, "[shell] [shell] [editor /r/b.rs] [editor /r/x.rs, editor /r/a.rs]*"),
        ] {
            let seen = in_the_open(&[
                TWO_PANES_ON_A_FILE,
                &format!(
                    r#"
                const {{ running }} = two({dormant_first}, {{ path: "/r/a.rs", on: null, opened: 1 }});
                rename("/r/a.rs", "/r/b.rs"); moved("/r/a.rs", "/r/b.rs", null); flush();
                const renamed = strip("s1");
                openFile("/r/b.rs", null); flush();
                return [renamed, focused().pane === running ? "the started pane" : "the other"];
            "#
                ),
            ]
            .join("\n"));
            assert_eq!(seen, [strip, "the started pane"], "the waiting pane first: {dormant_first}");
        }
    }

    #[test]
    fn a_pane_whose_document_was_renamed_is_not_given_the_document_of_another_file_at_its_old_name() {
        let seen = in_the_open(
            r#"
            const out = [];
            openFile("/r/a.rs", null); flush();
            const stale = focused().pane;
            run("pane.focus", { pane: "p1" }); flush();
            rename("/r/a.rs", "/r/b.rs"); moved("/r/a.rs", "/r/b.rs", null); flush();
            // A file made under the old name is a new one, in a tab ahead of the pane that holds the renamed.
            openFile("/r/a.rs", null); flush();
            const made = focused().pane;
            run("pane.focus", { pane: "p1" }); flush();
            rename("/r/a.rs", "/r/c.rs"); moved("/r/a.rs", "/r/c.rs", null); flush();
            openFile("/r/c.rs", null); flush();
            out.push(focused().pane === made ? "the pane of the new file" : focused().pane === stale ? "the pane of the renamed" : focused().pane);
            openFile("/r/b.rs", null); flush();
            out.push(focused().pane === stale ? "the pane of the renamed" : focused().pane);
            return out;
        "#,
        );
        assert_eq!(seen, ["the pane of the new file", "the pane of the renamed"]);
    }

    #[test]
    fn a_pane_that_has_started_is_shown_ahead_of_one_that_has_not_when_both_hold_the_file() {
        let seen = in_the_open(&[
            TWO_PANES_ON_A_FILE,
            r#"
            const { running } = two(true, { path: "/r/a.rs", on: null, opened: 1 });
            openFile("/r/a.rs", null, { line: 12, column: 4 }); flush();
            return [focused().pane === running ? "the started pane" : "the other", JSON.stringify(revealed), strip("s1")];
        "#,
        ]
        .join("\n"));
        assert_eq!(
            seen,
            [
                "the started pane",
                "[[12,4]]",
                "[shell] [shell] [editor /r/a.rs] [editor /r/x.rs, editor /r/a.rs]*",
            ]
        );
    }

    #[test]
    fn a_line_asked_of_a_file_in_a_pane_not_started_is_given_to_the_pane_that_takes_its_place() {
        let seen = in_the_open(
            r#"
            restore("s1", "", "yuzora-editor", { path: "/r/a.rs", on: null, opened: 1 }, { beside: true });
            restore("s1", "b.rs", "yuzora-editor", { path: "/r/b.rs", on: null, opened: 1, line: 3 });
            openFile("/r/a.rs", null, { line: 40, column: 2 }); flush();
            const asked = JSON.stringify(revealed);
            const before = paneOf("/r/b.rs");
            openFile("/r/b.rs", null, { line: 7 }); flush();
            const now = focused().pane, at = given[now];
            return [
              asked, JSON.stringify(revealed), `${at.path} ${at.line}:${at.column}`, String(at.opened > 1),
              String(now === before), String(panes().some(p => p.pane === before)), focus(), strip("s1"),
            ];
        "#,
        );
        assert_eq!(
            seen,
            [
                "[[40,2]]",
                "[[40,2]]",
                "/r/b.rs 7:1",
                "true",
                "false",
                "false",
                "editor /r/b.rs",
                "[shell, editor /r/a.rs] [shell] [editor /r/b.rs]*",
            ]
        );
    }

    #[test]
    fn a_line_asked_of_a_markdown_file_in_a_pane_not_started_of_a_remote_space_is_given_to_the_pane_that_takes_its_place() {
        let seen = in_the_open(
            r#"
            restore("s2", "R.md", "yuzora-markdown", { path: "/srv/R.md", on: "lab", opened: 1 }, { pane: "lab/pm1" });
            show("s2");
            openFile("/srv/R.md", "lab", { line: 5, column: 3 }); flush();
            const at = given[focused().pane];
            return [shown().id, `${at.on}:${at.path} ${at.line}:${at.column}`, strip("s1"), strip("s2")];
        "#,
        );
        assert_eq!(
            seen,
            [
                "s2",
                "lab:/srv/R.md 5:3",
                "[shell]* [shell]",
                "[shell] [markdown /srv/R.md]*",
            ]
        );
    }

    #[test]
    fn of_two_lines_asked_of_one_file_in_a_pane_not_started_the_last_is_the_one_gone_to() {
        let seen = in_the_open(
            r#"
            restore("s1", "b.rs", "yuzora-editor", { path: "/r/b.rs", on: null, opened: 1 });
            openFile("/r/b.rs", null, { line: 7 }); flush();
            const first = given[focused().pane].line;
            openFile("/r/b.rs", null, { line: 9, column: 2 }); flush();
            return [String(first), JSON.stringify(revealed), strip("s1")];
        "#,
        );
        assert_eq!(seen, ["7", "[[9,2]]", "[shell] [shell] [editor /r/b.rs]*"]);
    }

    #[test]
    fn a_file_asked_of_with_no_line_in_a_pane_not_started_is_shown_where_it_is() {
        let seen = in_the_open(
            r#"
            restore("s1", "b.rs", "yuzora-editor", { path: "/r/b.rs", on: null, opened: 1 });
            const before = paneOf("/r/b.rs");
            openFile("/r/b.rs", null); flush();
            return [String(focused().pane === before), strip("s1")];
        "#,
        );
        assert_eq!(seen, ["true", "[shell] [shell] [editor /r/b.rs]*"]);
    }

    #[test]
    fn an_image_restored_with_the_layout_is_replaced_under_its_new_path_when_renamed() {
        let seen = in_the_open(
            r#"
            const out = [];
            restore("s1", "", "yuzora-files-image", { path: "/r/a.png", on: null, opened: 1 }, { beside: true });
            restore("s1", "c.png", "yuzora-files-image", { path: "/r/img/c.png", on: null, opened: 1 });
            moved("/r/a.png", "/r/b.png", null); flush();
            out.push(strip("s1") + " " + focus());
            openFile("/r/b.png", null); flush();
            out.push(strip("s1") + " " + focus());
            moved("/r/img", "/r/pics", null); flush();
            run("pane.focus", { pane: "p1" }); flush();
            openFile("/r/pics/c.png", null); flush();
            out.push(strip("s1") + " " + focus());
            return out;
        "#,
        );
        assert_eq!(
            seen,
            [
                "[shell, image /r/b.png]* [shell] [image /r/img/c.png] shell",
                "[shell, image /r/b.png]* [shell] [image /r/img/c.png] image /r/b.png",
                "[shell, image /r/b.png] [shell] [image /r/pics/c.png]* image /r/pics/c.png",
            ]
        );
    }

    #[test]
    fn an_agent_folder_holds_its_files_with_either_slash_on_windows() {
        let src = include_str!("../../../defaults/mods/yuzora-editor/doc-pane.js");
        // A top-level function of doc-pane.js, as written there.
        let function = |name: &str| {
            let start = src.find(&format!("\nfunction {name}(")).unwrap();
            let end = start + src[start..].find("\n}\n").unwrap();
            &src[start..end + 2]
        };
        let cases = [
            (r"C:\repo\src\a.rs", r"C:\repo", true),
            // The sidebar joins with `/` under a native root.
            (r"C:\repo/src/a.rs", r"C:\repo", true),
            (r"C:/repo/src/a.rs", r"C:\Repo\", true),
            (r"c:\repo", r"C:\repo", true),
            (r"\\srv\share\x\a.md", r"\\srv\share", true),
            (r"C:\repository\a.rs", r"C:\repo", false),
            (r"D:\repo\a.rs", r"C:\repo", false),
            ("/home/a/b.rs", "/home/a", true),
            ("/home/a/b.rs", "/home/a/", true),
            ("/home/ab.rs", "/home/a", false),
            // Elsewhere a backslash is part of a name, and case counts.
            (r"/home/a\b.rs", "/home/a", false),
            ("/Home/a/b.rs", "/home/a", false),
        ];
        let calls: Vec<String> = cases
            .iter()
            .map(|(path, dir, _)| format!("within({path:?}, {dir:?})"))
            .collect();
        let script = format!(
            "(() => {{ {}{}
            const AGENT_WINDOW = 60000;
            const panes = () => [{{ pane: 'p1', cwd: 'C:\\\\Users\\\\me\\\\repo' }}];
            const agents = () => [{{ pane: 'p1', status: 'working', name: 'codex' }}];
            const editing = path => editingAgent({{ externalAt: Date.now(), path }})?.name ?? null;
            return JSON.stringify([[{}], editing('C:\\\\Users\\\\me\\\\repo\\\\src\\\\a.rs'), editing('C:\\\\Users\\\\me\\\\other\\\\a.rs')]);
            }})()",
            function("within"),
            function("editingAgent"),
            calls.join(", ")
        );
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let out: String = context.with(|ctx| ctx.eval(script).unwrap());
        let (got, agent, none): (Vec<bool>, Option<String>, Option<String>) =
            serde_json::from_str(&out).unwrap();
        let want: Vec<bool> = cases.iter().map(|(.., want)| *want).collect();
        assert_eq!(got, want);
        assert_eq!((agent.as_deref(), none), (Some("codex"), None));
    }

    // The panes watch their file's folder and match what the watch reports with the path
    // they opened: both need Windows paths (`C:\\…`, either slash, any case), not only `/`.
    #[test]
    fn the_panes_follow_files_by_windows_paths_too() {
        let sources = [
            include_str!("../../../defaults/mods/yuzora-editor/doc-pane.js"),
            include_str!("../../../defaults/mods/yuzora-files-image/main.js"),
        ];
        for src in sources {
            let function = |name: &str| {
                let start = src.find(&format!("\nfunction {name}(")).unwrap();
                let end = start + src[start..].find("\n}\n").unwrap();
                &src[start..end + 2]
            };
            let script = format!(
                r#"(() => {{ {}{}{}{}{}
                const parents = [{p}].map(parent);
                const roots = [{r}].map(isRoot);
                const names = [{n}].map(baseName);
                const same = [{s}].map(([a, b]) => samePath(a, b));
                return JSON.stringify([parents, roots, names, same]);
                }})()"#,
                function("isWin"),
                function("parent"),
                function("isRoot"),
                function("baseName"),
                function("samePath"),
                p = PARENTS.iter().map(|(p, _)| format!("{p:?}")).collect::<Vec<_>>().join(", "),
                r = ROOTS.iter().map(|(p, _)| format!("{p:?}")).collect::<Vec<_>>().join(", "),
                n = NAMES.iter().map(|(p, _)| format!("{p:?}")).collect::<Vec<_>>().join(", "),
                s = SAME.iter().map(|(a, b, _)| format!("[{a:?}, {b:?}]")).collect::<Vec<_>>().join(", "),
            );
            let runtime = quickjs::Runtime::new().unwrap();
            let context = quickjs::Context::full(&runtime).unwrap();
            let out: String = context.with(|ctx| ctx.eval(script).unwrap());
            let (parents, roots, names, same): (Vec<String>, Vec<bool>, Vec<String>, Vec<bool>) =
                serde_json::from_str(&out).unwrap();
            assert_eq!(parents, PARENTS.iter().map(|(_, w)| w.to_string()).collect::<Vec<_>>());
            assert_eq!(roots, ROOTS.iter().map(|(_, w)| *w).collect::<Vec<_>>());
            assert_eq!(names, NAMES.iter().map(|(_, w)| w.to_string()).collect::<Vec<_>>());
            assert_eq!(same, SAME.iter().map(|(.., w)| *w).collect::<Vec<_>>());
        }
    }

    const PARENTS: [(&str, &str); 9] = [
        ("/home/a/b.txt", "/home/a"),
        ("/b.txt", ""),
        (r"C:\Users\me\a b.txt", r"C:\Users\me"),
        (r"C:\Users/me/a.txt", r"C:\Users/me"),
        (r"C:\a.txt", r"C:\"),
        (r"C:/a.txt", "C:/"),
        (r"\\srv\share\x\a.md", r"\\srv\share\x"),
        // Elsewhere a backslash belongs to the name.
        (r"/home/a\b.txt", "/home"),
        ("rel.txt", ""),
    ];
    const ROOTS: [(&str, bool); 6] = [
        ("/", true),
        (r"C:\", true),
        ("c:/", true),
        ("C:", true),
        (r"C:\Users", false),
        ("/home", false),
    ];
    const NAMES: [(&str, &str); 4] = [
        ("/home/a/b.png", "b.png"),
        (r"C:\pics\a b.png", "a b.png"),
        (r"C:\pics/a.png", "a.png"),
        (r"/home/a\b.png", r"a\b.png"),
    ];
    const SAME: [(&str, &str, bool); 7] = [
        ("/a/b", "/a/b", true),
        ("/a/b", "/a/B", false),
        (r"C:\Users\me\a.txt", r"c:\users\me\A.TXT", true),
        (r"C:\Users\me\a.txt", "C:/Users/me/a.txt", true),
        (r"C:\Users\me", r"C:\Users\me\", true),
        (r"C:\Users\me\a.txt", r"\\?\C:\Users\me\a.txt", false),
        (r"C:\", "C:/", true),
    ];

    // Code and its language's badge read on both Yuzora themes: AA on the light
    // one; on the dark one, no less than One Dark and the old badge colors did.
    #[test]
    fn code_reads_on_both_yuzora_themes() {
        use gpui::Hsla;
        use gpui_base::input::HighlightStyleResolver as _;

        use crate::highlight::{SyntaxPalette, parse_hex};
        let src = include_str!("../../../defaults/mods/yuzora-editor/doc-pane.js");
        let part = |start: &str, end: &str| {
            let at = src.find(start).unwrap();
            &src[at..at + src[at..].find(end).unwrap() + end.len()]
        };
        let script = |th: &str| {
            format!(
                // As mods run: strict, with the built-in prototypes frozen.
                "(() => {{ 'use strict'; Object.freeze(Object.prototype); {}{}{} const th = {th}; return JSON.stringify([syntax(th), mix(th.hover, th.background, 0.5)]); }})()",
                part("\nconst SYNTAX = {", "\n};\n"),
                part("\nfunction syntax(", "\n}\n"),
                part("\nfunction mix(", "\n}\n"),
            )
        };
        let lum = |c: Hsla| {
            let c = c.to_rgb();
            let ch = |v: f32| {
                let v = v as f64;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        };
        let contrast = |a: Hsla, b: Hsla| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let hex = |s: &str| parse_hex(s).unwrap();
        let one_dark = SyntaxPalette::one_dark();
        // Before the badges took theme colors.
        let old_badge = |lang: Lang| match lang {
            Lang::Rust | Lang::Html => "#f08a5d",
            Lang::TypeScript | Lang::Tsx | Lang::Python => "#60a5fa",
            Lang::JavaScript | Lang::Json => "#e5c07b",
            Lang::Toml => "#9ca3af",
            Lang::Yaml => "#f87171",
            Lang::Markdown => "#93c5fd",
            Lang::Go => "#7dd3fc",
            Lang::Sql => "#a78bfa",
            Lang::Bash => "#86d17a",
            Lang::Css => "#f472b6",
        };
        for (file, light) in [
            (
                include_str!("../../../defaults/themes/yuzora-dark.json"),
                false,
            ),
            (
                include_str!("../../../defaults/themes/yuzora-light.json"),
                true,
            ),
        ] {
            // As `theme()` gives it: the colors and ansi0–ansi15.
            let theme: serde_json::Value = serde_json::from_str(file).unwrap();
            let mut th = theme["colors"].as_object().unwrap().clone();
            for (i, a) in theme["ansi"].as_array().unwrap().iter().enumerate() {
                th.insert(format!("ansi{i}"), a.clone());
            }
            let c = |k: &str| hex(th[k].as_str().unwrap());
            let runtime = quickjs::Runtime::new().unwrap();
            let context = quickjs::Context::full(&runtime).unwrap();
            let out: String = context.with(|ctx| {
                ctx.eval(script(&serde_json::Value::from(th.clone()).to_string()))
                    .unwrap()
            });
            let (code, mixed): (HashMap<String, String>, String) =
                serde_json::from_str(&out).unwrap();
            let mut names: Vec<&str> = code.keys().map(String::as_str).collect();
            let mut colored = one_dark.colored();
            names.sort_unstable();
            colored.sort_unstable();
            assert_eq!(
                names, colored,
                "every name One Dark colors takes a theme color"
            );
            // Code on the editor, a Markdown code block and the current line.
            let active = if light { hex(&mixed) } else { c("hover") };
            for (name, color) in &code {
                for (bg, on) in [
                    ("background", c("background")),
                    ("raised", c("raised")),
                    ("current line", active),
                ] {
                    let r = contrast(hex(color), on);
                    if light {
                        assert!(r >= 4.5, "light: {name} on {bg} is {r:.2}");
                    } else {
                        let was = contrast(one_dark.style(name).unwrap().color.unwrap(), on);
                        assert!(
                            r + 0.01 >= was,
                            "dark: {name} on {bg} is {r:.2}, One Dark's {was:.2}"
                        );
                    }
                }
            }
            for lang in Lang::all() {
                let r = contrast(c(lang.badge_color()), c("background"));
                if light {
                    assert!(r >= 4.5, "light: {} badge is {r:.2}", lang.id());
                } else {
                    let was = contrast(hex(old_badge(*lang)), c("background"));
                    assert!(
                        r + 0.01 >= was,
                        "dark: {} badge is {r:.2}, was {was:.2}",
                        lang.id()
                    );
                }
            }
        }
    }

    #[test]
    fn doc_pane_mixes_two_colors() {
        let src = include_str!("../../../defaults/mods/yuzora-editor/doc-pane.js");
        let at = src.find("\nfunction mix(").unwrap();
        let mix = &src[at..at + src[at..].find("\n}\n").unwrap() + 3];
        let script = format!(
            "(() => {{ {mix} return JSON.stringify([mix('#000000', '#ffffff', 0.5), mix('#e3e7f2', '#FFFFFF', 0.5), mix('#102030', '#000000', 1), mix('#00000000', '#ffffff', 0.5), mix('#ffffff', undefined, 0.5)]); }})()"
        );
        let runtime = quickjs::Runtime::new().unwrap();
        let context = quickjs::Context::full(&runtime).unwrap();
        let out: String = context.with(|ctx| ctx.eval(script).unwrap());
        let got: Vec<String> = serde_json::from_str(&out).unwrap();
        // Not two #rrggbb: the first as it is.
        assert_eq!(
            got,
            ["#808080", "#f1f3f9", "#102030", "#00000000", "#ffffff"]
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
}
