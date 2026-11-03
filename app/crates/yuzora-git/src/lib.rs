//! Yuzora L2（Git）: Git, modelled on IntelliJ IDEA Community: status, staging, commit, branches, log graph, diff and merge.
//!
//! Owned by lane L2. Scripts reach it as `import { … } from "yuzora:git"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].
//!
//! Layout (see docs/beta2/git.md):
//! - [`exec`]: running git (environment, locks, cancel, timeout, remote);
//! - [`repo`]: opened repos, change generations, `.git` state;
//! - [`status`] + [`watch`]: status by dirty scope, the file watcher;
//! - [`diff`] + [`diffview`]: Myers/Patience diff, the diff pane, partial staging;
//! - [`log`] + [`graph`] + [`graph_view`]: two-phase log, the graph and its element;
//! - [`ops`]: commit, branches, remotes, stash, history, tags, patches…;
//! - [`merge`]: three-way conflicts; [`rebase`]: interactive rebase;
//! - [`settings`]: the `git` section of Yuzora's settings.

pub mod diff;
pub mod diffview;
pub mod exec;
pub mod graph;
pub mod graph_view;
pub mod log;
pub mod lru;
pub mod merge;
pub mod ops;
pub mod rebase;
pub mod repo;
pub mod settings;
pub mod status;
pub mod trust;
pub mod watch;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::IntoElement as _;
use serde_json::{Value, json};
use yuzora_core::host::{arg, from_host, to_host};
use yuzora_core::{HostArguments, HostError, HostModule, HostValue};

use crate::exec::{Cancel, GitError, GitResult};
use crate::repo::Repo;

/// The import specifier: `yuzora:git`.
pub const NAME: &str = "yuzora:git";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
export interface RepoInfo { id: string; root: string; machine: string | null; gitDir: string; commonDir: string }
export interface FileChange { path: string; orig?: string | null; kind: string; add: number | null; del: number | null; binary: boolean; submodule?: string | null }
export interface Status {
  repo: string; root: string; machine: string | null; generation: number;
  branch: { head: string | null; oid: string | null; upstream: string | null; ahead: number; behind: number; detached: boolean; initial: boolean };
  operation: { kind: string; step: [number, number] | null; onto: string | null; head: string | null; interactive: boolean };
  staged: FileChange[]; unstaged: FileChange[]; untracked: FileChange[]; conflicts: { path: string; kind: string }[];
  counts: { staged: number; unstaged: number; untracked: number; conflicts: number; total: number };
  stash: number; truncated: boolean; numstatSkipped: boolean;
  /** Set while the repo's own config names programs (filters, ssh, credentials) and its folder isn't trusted: no git ran. */
  trust?: string[];
}
export interface Outcome { ok: boolean; output?: string; conflicts?: string[]; operation?: string; [k: string]: unknown }
export interface LogFilter { branches?: string[]; users?: string[]; since?: string; until?: string; paths?: string[]; text?: string; regex?: boolean; matchCase?: boolean; hashes?: string[]; range?: string; noMerges?: boolean; follow?: boolean }
export interface LogInfo { id: number; generation: number; rows: number; loading: boolean; stage: string; complete: boolean; error: string | null; graph: boolean; width: number; firstMs: number | null; fullMs: number | null; branch: string | null }
export interface LogRow { row: number; hash: string; short: string; subject: string | null; author: string | null; email: string | null; time: number | null; refs: { name: string; kind: "head" | "local" | "remote" | "tag" }[]; head: boolean; merge: boolean }
/** The yuzora-git crate's version (Yuzora's). */
export function version(): string;
/** The repository holding cwd (here, or on machine `on`): opened once, watched while open. */
export function open(cwd: string, on?: string | null): Promise<RepoInfo>;
/** Status, refreshed by dirty scope (force: read everything again). */
export function status(repo: string, options?: { force?: boolean }): Promise<Status>;
/** Trusts the repo's folder: git may run the programs its own config names (see Status.trust). */
export function trust(repo: string): Promise<Outcome>;
/** Resolves with the repo's generation once it is past `since` (at most ~25 s). */
export function changed(repo: string, since: number): Promise<number>;
/** git add -A of paths (all when empty). */
export function stage(repo: string, paths: string[]): Promise<Outcome>;
/** Paths back out of the index (all when empty). */
export function unstage(repo: string, paths: string[]): Promise<Outcome>;
/** Work tree changes thrown away (untracked files deleted); rollback also resets the index to HEAD. */
export function discard(repo: string, paths: string[], rollback?: boolean): Promise<Outcome>;
/** Warnings before a commit: identity, conflicts, detached, nothing, large, crlf. */
export function commitChecks(repo: string, amend?: boolean): Promise<{ kind: string; files?: string[]; count?: number }[]>;
export function commit(repo: string, options: { message: string; amend?: boolean; signOff?: boolean; noVerify?: boolean; author?: string; date?: string; all?: boolean }): Promise<Outcome & { hash?: string }>;
/** Message history (this repo), commit.template, the last message, recent authors. */
export function messages(repo: string): Promise<{ history: string[]; template: string | null; last: string | null; authors: string[] }>;
/** Loads a file diff: mode unstaged | staged | commit (rev) | compare (rev, to: null = work tree). */
export function diffOpen(repo: string, spec: { path: string; orig?: string | null; mode: string; rev?: string; to?: string | null }): Promise<{ id: number }>;
/** The rows of a loaded diff: layout side | unified, context lines (-1 all), whitespace none | trim | all. */
export function diffView(id: number, options?: { layout?: string; context?: number; whitespace?: string }): Promise<any>;
/** Stages whole hunks, or lines of hunks ({hunk: {del: [line], add: [line]}}). */
export function diffStage(id: number, hunks: number[], lines?: Record<string, { del?: number[]; add?: number[] }>): Promise<Outcome>;
export function diffUnstage(id: number, hunks: number[], lines?: Record<string, { del?: number[]; add?: number[] }>): Promise<Outcome>;
export function diffDiscard(id: number, hunks: number[], lines?: Record<string, { del?: number[]; add?: number[] }>): Promise<Outcome>;
/** Opens a log (empty filter: everything) and starts loading it. */
export function logOpen(repo: string, filter?: LogFilter): number;
export function logInfo(id: number): LogInfo;
/** Rows start..end; missing metadata is fetched and logWait resolves when it arrives. */
export function logRows(id: number, start: number, end: number): LogRow[];
/** Resolves once the log changed past `since` (or the repo's refs moved, which refreshes it). */
export function logWait(id: number, since: number): Promise<number>;
/** The row of a hash (prefix) or ref name. */
export function logFind(id: number, query: string): number | null;
/** Reads everything again. */
export function logRefresh(id: number): void;
export function logClose(id: number): void;
/** One commit: message, people, signature (%G?), changed files. */
export function commitDetails(repo: string, hash: string): Promise<any>;
/** One graph cell: props {log, row, head?, width?, lane?, height?, background?, colors?}. */
export const GraphCell: { new(id: string, props: Record<string, unknown>): import("gpui-kit").NativeElement };
export function branches(repo: string): Promise<any>;
/** Branch, remote branch (tracking branch made), tag or commit; smart: stash and restore local changes. */
export function checkout(repo: string, target: string, options?: { smart?: boolean; force?: boolean }): Promise<Outcome>;
export function branchCreate(repo: string, name: string, start?: string | null, checkout?: boolean): Promise<Outcome>;
export function branchRename(repo: string, from: string, to: string): Promise<Outcome>;
export function branchDelete(repo: string, name: string, options?: { force?: boolean; remote?: boolean; token?: string }): Promise<Outcome>;
export function branchTrack(repo: string, branch: string, upstream: string | null): Promise<Outcome>;
/** Commits only on a, only on b, and the files that differ (a...b). */
export function compare(repo: string, a: string, b: string): Promise<any>;
export function merge(repo: string, target: string, options?: { noFf?: boolean; ffOnly?: boolean; squash?: boolean; noCommit?: boolean; message?: string; noVerify?: boolean; allowUnrelated?: boolean }): Promise<Outcome>;
export function rebase(repo: string, upstream: string, options?: { onto?: string; branch?: string; rebaseMerges?: boolean; updateRefs?: boolean; autosquash?: boolean; keepEmpty?: boolean; root?: boolean }): Promise<Outcome>;
/** continue | abort | skip | quit for the merge, rebase, cherry-pick or revert in progress. */
export function sequencer(repo: string, action: string): Promise<Outcome>;
export function fetch(repo: string, options?: { remote?: string; prune?: boolean; token?: string; auto?: boolean }): Promise<Outcome>;
export function pull(repo: string, options?: { remote?: string; branch?: string; method?: string; noFf?: boolean; squash?: boolean; noCommit?: boolean; noVerify?: boolean; token?: string }): Promise<Outcome>;
/** Where the current branch would push and the commits it would send. */
export function pushPreview(repo: string): Promise<any>;
export function push(repo: string, options?: { remote?: string; branch?: string; target?: string; force?: "" | "lease" | "force"; tags?: "none" | "follow" | "all"; setUpstream?: boolean; noVerify?: boolean; autoUpdate?: boolean; token?: string }): Promise<Outcome>;
/** Stops the call started with this token (fetch, pull, push, remote deletes). */
export function cancel(token: string): void;
export function remotes(repo: string): Promise<{ name: string; url: string; pushUrl: string }[]>;
/** add | set-url | rename | remove. */
export function remoteEdit(repo: string, action: string, name: string, value?: string | null): Promise<Outcome>;
export function stashes(repo: string): Promise<{ ref: string; hash: string; time: number; message: string }[]>;
export function stashPush(repo: string, options?: { message?: string; keepIndex?: boolean; untracked?: boolean; paths?: string[] }): Promise<Outcome>;
/** apply | pop (index: restore the index too) | drop | branch (branch name) | clear. */
export function stashAction(repo: string, action: string, stash: string, options?: { index?: boolean; branch?: string }): Promise<Outcome>;
export function cherryPick(repo: string, hashes: string[], options?: { suffix?: boolean; noCommit?: boolean }): Promise<Outcome>;
export function revert(repo: string, hashes: string[], options?: { noCommit?: boolean }): Promise<Outcome>;
/** soft | mixed | hard | keep. */
export function reset(repo: string, rev: string, mode: string): Promise<Outcome>;
/** The last commit undone, its changes kept staged. */
export function undoCommit(repo: string): Promise<Outcome>;
export function tagCreate(repo: string, name: string, rev?: string | null, message?: string | null): Promise<Outcome>;
export function tagDelete(repo: string, name: string, remote?: string | null, token?: string): Promise<Outcome>;
export function tagPush(repo: string, remote: string, name?: string | null, token?: string): Promise<Outcome>;
/** A patch: of commits (hashes), or of staged / unstaged changes in paths. */
export function patchCreate(repo: string, spec: { hashes?: string[]; paths?: string[]; staged?: boolean }): Promise<string>;
/** Writes a patch file into a folder here; returns its path. */
export function patchSave(repo: string, spec: { hashes?: string[]; paths?: string[]; staged?: boolean }, folder: string, name: string): Promise<string>;
/** git apply: check, cached, index, threeWay, reverse. */
export function patchApply(repo: string, text: string, options?: { check?: boolean; cached?: boolean; index?: boolean; threeWay?: boolean; reverse?: boolean }): Promise<Outcome>;
export function worktrees(repo: string): Promise<any[]>;
/** lock | unlock | remove | prune. */
export function worktreeAction(repo: string, action: string, path?: string | null, force?: boolean): Promise<Outcome>;
export function submodules(repo: string): Promise<{ path: string; hash: string; state: string; describe: string | null }[]>;
export function submoduleUpdate(repo: string, options?: { token?: string }): Promise<Outcome>;
/** Identity, signing and line-ending config of this repo, and git's version. */
export function config(repo: string): Promise<any>;
/** user.name, user.email, commit.gpgSign, user.signingKey, gpg.format, core.autocrlf (null unsets). */
export function setConfig(repo: string, key: string, value: string | null, global?: boolean): Promise<Outcome>;
export function blame(repo: string, path: string, rev?: string | null, options?: { ignoreWhitespace?: boolean; movement?: string }): Promise<any>;
/** One level of a revision's tree. */
export function tree(repo: string, rev: string, dir?: string): Promise<any[]>;
/** A file's text at a revision (null when missing, binary or too large). */
export function fileAt(repo: string, rev: string, path: string): Promise<string | null>;
/** A conflicted file: chunks (equal | ours | theirs | same | conflict) with sides swapped during a rebase. */
export function conflictOpen(repo: string, path: string): Promise<any>;
/** Writes the result ({chunk: "ours" | "theirs" | "both" | "both-theirs-first" | "base" | "auto" | {text}}) and git adds it. */
export function conflictResolve(id: number, choices: Record<string, unknown>): Promise<Outcome>;
/** Whole files: yours (ours) or theirs, as displayed. */
export function conflictAccept(repo: string, paths: string[], side: "ours" | "theirs"): Promise<Outcome>;
/** The commits base..HEAD oldest first, and branches pointing into them. */
export function rebaseTodo(repo: string, base: string): Promise<any>;
/** git rebase -i with this todo: {action: pick | reword | edit | squash | fixup | drop | update-ref, hash | ref, subject, message?}. */
export function rebaseInteractive(repo: string, base: string, entries: Record<string, unknown>[], options?: { updateRefs?: boolean }): Promise<Outcome & { stopped?: boolean }>;
/** A message rewritten without touching the work tree or index (linear history only). */
export function rewordInMemory(repo: string, hash: string, message: string): Promise<Outcome>;
/** The git section of Yuzora's settings, defaults filled in. */
export function settings(): Record<string, any>;
/** Merges values into the git settings (known keys only); refuses a protected-branches pattern the engine can't read. */
export function setSettings(values: Record<string, unknown>): void;
/** Why the engine can't read a regular expression (its dialect, not JavaScript's), or null. */
export function checkPattern(pattern: string): string | null;
"#;

fn err(e: impl std::fmt::Display) -> HostError {
    HostError::new(e.to_string())
}

fn cancels() -> &'static Mutex<HashMap<String, Cancel>> {
    static C: OnceLock<Mutex<HashMap<String, Cancel>>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

/// A cancel handle for the call's `token` option (removed when the call ends).
fn cancel_for(o: &Value) -> (Option<Cancel>, Option<String>) {
    match o.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        Some(t) => {
            let c = Cancel::new();
            cancels().lock().unwrap_or_else(|e| e.into_inner()).insert(t.to_owned(), c.clone());
            (Some(c), Some(t.to_owned()))
        }
        None => (None, None),
    }
}

fn forget_cancel(token: Option<String>) {
    if let Some(t) = token {
        cancels().lock().unwrap_or_else(|e| e.into_inner()).remove(&t);
    }
}

fn args_json(a: &HostArguments, from: usize) -> Vec<Value> {
    (from..a.len().max(from)).map(|i| arg(a, i)).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
}

fn s(v: &Value) -> Option<&str> {
    v.as_str().filter(|x| !x.is_empty())
}

/// Resolves a host promise from work on its own thread.
fn finish(name: &'static str, work: impl FnOnce() -> GitResult<Value> + Send + 'static) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send + 'static {
    let fut = exec::spawn(name, work);
    async move {
        match fut.await {
            Some(Ok(v)) => Ok(to_host(&v)),
            Some(Err(e)) => Err(err(e)),
            None => Err(HostError::new("the git task was lost")),
        }
    }
}

type RepoJob = Arc<dyn Fn(Arc<Repo>, Vec<Value>) -> GitResult<Value> + Send + Sync>;

/// An async function whose first argument is a repo id.
fn repo_fn(m: HostModule, name: &'static str, job: impl Fn(Arc<Repo>, Vec<Value>) -> GitResult<Value> + Send + Sync + 'static) -> HostModule {
    let job: RepoJob = Arc::new(job);
    m.async_function(name, move |a| {
        let repo = repo::get(a.string(0)?).map_err(err)?;
        let rest = args_json(a, 1);
        let job = job.clone();
        Ok(finish(name, move || {
            // `status` shows the trust prompt itself; `trust` answers it.
            if !matches!(name, "status" | "trust") {
                trust::check(&repo)?;
                status::refuse_lossy(&repo, &rest)?;
            }
            job(repo, rest)
        }))
    })
}

/// Like [`repo_fn`], with a cancel token from the options at `opt`.
fn repo_fn_cancellable(m: HostModule, name: &'static str, opt: usize, job: impl Fn(Arc<Repo>, Vec<Value>, Option<Cancel>) -> GitResult<Value> + Send + Sync + 'static) -> HostModule {
    let job = Arc::new(job);
    m.async_function(name, move |a| {
        let repo = repo::get(a.string(0)?).map_err(err)?;
        let rest = args_json(a, 1);
        let o = rest.get(opt).cloned().unwrap_or(Value::Null);
        let (cancel, token) = cancel_for(&if o.is_string() { json!({ "token": o }) } else { o });
        let job = job.clone();
        Ok(finish(name, move || {
            let r = trust::check(&repo).and_then(|_| status::refuse_lossy(&repo, &rest)).and_then(|_| job(repo, rest, cancel));
            forget_cancel(token);
            r
        }))
    })
}

fn selection(rest: &[Value]) -> diffview::Selection {
    diffview::Selection::from_json(rest.first().unwrap_or(&Value::Null), rest.get(1).unwrap_or(&Value::Null))
}

fn diff_fn(m: HostModule, name: &'static str, job: fn(&diffview::DiffSession, &diffview::Selection) -> GitResult<()>) -> HostModule {
    m.async_function(name, move |a| {
        let s = diffview::get(a.integer(0)? as u64).map_err(err)?;
        let rest = args_json(a, 1);
        Ok(finish(name, move || job(&s, &selection(&rest)).map(|_| json!({ "ok": true }))))
    })
}

/// The `yuzora:git` module.
pub fn host_module() -> HostModule {
    let mut m = HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
        .async_function("open", |a| {
            let cwd = a.string(0)?.to_owned();
            let on = a.get(1).and_then(HostValue::as_str).map(str::to_owned).filter(|m| !m.is_empty());
            Ok(finish("open", move || {
                let r = repo::open(&cwd, on.as_deref())?;
                Ok(json!({ "id": r.id, "root": r.place.root, "machine": r.place.on, "gitDir": r.git_dir, "commonDir": r.common_dir }))
            }))
        })
        .async_function("changed", |a| {
            let repo = repo::get(a.string(0)?).map_err(err)?;
            let since = a.number(1).unwrap_or(0.0) as u64;
            let wait = repo.wait(since);
            Ok(async move { Ok(HostValue::Number(wait.await as f64)) })
        })
        .function("cancel", |a| {
            if let Some(c) = cancels().lock().unwrap_or_else(|e| e.into_inner()).get(a.string(0)?) {
                c.cancel();
            }
            Ok(HostValue::Null)
        })
        .function("settings", |_| Ok(to_host(&settings::all())))
        .function("setSettings", |a| {
            let values = arg(a, 0);
            let known = settings::defaults();
            let mut current = match yuzora_core::settings::section_from(&settings::file(), settings::SECTION) {
                Value::Object(o) => o,
                _ => serde_json::Map::new(),
            };
            if let Value::Object(v) = values {
                for (k, x) in v {
                    if k == "protectedBranches"
                        && let Some(e) = x.as_str().and_then(settings::pattern_error)
                    {
                        return Err(err(GitError::Other(format!("not a regular expression git can use: {e}"))));
                    }
                    if known.get(&k).is_some() {
                        current.insert(k, x);
                    }
                }
            }
            yuzora_core::settings::set_section_at(&settings::file(), settings::SECTION, Value::Object(current)).map_err(err)?;
            Ok(HostValue::Null)
        })
        .function("checkPattern", |a| Ok(settings::pattern_error(a.string(0)?).map_or(HostValue::Null, HostValue::Str)));

    m = repo_fn(m, "status", |r, a| status::read(&r, a.first().and_then(|o| o.get("force")).and_then(Value::as_bool).unwrap_or(false)));
    m = repo_fn(m, "trust", |r, _| trust::trust(&r));
    m = repo_fn(m, "stage", |r, a| ops::stage(&r, &strings(&a[0])));
    m = repo_fn(m, "unstage", |r, a| ops::unstage(&r, &strings(&a[0])));
    m = repo_fn(m, "discard", |r, a| ops::discard(&r, &strings(&a[0]), a.get(1).and_then(Value::as_bool).unwrap_or(false)));
    m = repo_fn(m, "commitChecks", |r, a| ops::commit_checks(&r, a.first().and_then(Value::as_bool).unwrap_or(false)));
    m = repo_fn(m, "commit", |r, a| ops::commit(&r, &ops::CommitOptions::from_json(&a[0])));
    m = repo_fn(m, "messages", |r, _| ops::messages(&r));
    m = repo_fn(m, "diffOpen", |r, a| {
        let spec = &a[0];
        let path = spec.get("path").and_then(Value::as_str).ok_or_else(|| GitError::other("diffOpen needs a path"))?.to_owned();
        let orig = spec.get("orig").and_then(Value::as_str).map(str::to_owned);
        let mode = diffview::Mode::parse(spec)?;
        let d = diffview::keep(diffview::DiffSession::load(r, path, orig, mode)?);
        Ok(json!({ "id": d.id }))
    });
    m = m.async_function("diffView", |a| {
        let d = diffview::get(a.integer(0)? as u64).map_err(err)?;
        let o = arg(a, 1);
        Ok(finish("diff", move || {
            let layout = o.get("layout").and_then(Value::as_str).unwrap_or("side").to_owned();
            let context = o.get("context").and_then(Value::as_i64).unwrap_or(3);
            let ws = diff::Whitespace::parse(o.get("whitespace").and_then(Value::as_str).unwrap_or("none"));
            Ok(d.view(&layout, context, ws))
        }))
    });
    m = diff_fn(m, "diffStage", diffview::DiffSession::stage);
    m = diff_fn(m, "diffUnstage", diffview::DiffSession::unstage);
    m = diff_fn(m, "diffDiscard", diffview::DiffSession::discard);

    m = m
        .function("logOpen", |a| {
            let repo = repo::get(a.string(0)?).map_err(err)?;
            let f = log::Filter::from_json(&arg(a, 1));
            Ok(HostValue::Number(log::open(repo, f).id as f64))
        })
        .function("logInfo", |a| Ok(to_host(&log::get(a.integer(0)? as u64).map_err(err)?.info())))
        .function("logRows", |a| {
            let s = log::get(a.integer(0)? as u64).map_err(err)?;
            Ok(to_host(&s.rows(a.integer(1)?.max(0) as usize, a.integer(2)?.max(0) as usize)))
        })
        .async_function("logWait", |a| {
            let s = log::get(a.integer(0)? as u64).map_err(err)?;
            let wait = s.wait(a.number(1).unwrap_or(0.0) as u64);
            Ok(async move { Ok(HostValue::Number(wait.await as f64)) })
        })
        .function("logFind", |a| Ok(log::get(a.integer(0)? as u64).map_err(err)?.find(a.string(1)?).map_or(HostValue::Null, |r| HostValue::Number(r as f64))))
        .function("logRefresh", |a| {
            log::get(a.integer(0)? as u64).map_err(err)?.start(false);
            Ok(HostValue::Null)
        })
        .function("logClose", |a| {
            log::close(a.integer(0)? as u64);
            Ok(HostValue::Null)
        })
        .component("GraphCell", |args, _window, _cx| graph_view::cell(args.props()).into_any_element());
    m = repo_fn(m, "commitDetails", |r, a| log::details(&r, s(&a[0]).ok_or_else(|| GitError::other("commitDetails needs a hash"))?));

    m = repo_fn(m, "branches", |r, _| ops::branches(&r));
    m = repo_fn(m, "checkout", |r, a| {
        let o = a.get(1).cloned().unwrap_or(Value::Null);
        ops::checkout(&r, s(&a[0]).ok_or_else(|| GitError::other("checkout needs a target"))?, o.get("smart").and_then(Value::as_bool).unwrap_or(true), o.get("force").and_then(Value::as_bool).unwrap_or(false))
    });
    m = repo_fn(m, "branchCreate", |r, a| ops::branch_create(&r, s(&a[0]).ok_or_else(|| GitError::other("a branch needs a name"))?, a.get(1).and_then(s), a.get(2).and_then(Value::as_bool).unwrap_or(false)));
    m = repo_fn(m, "branchRename", |r, a| ops::branch_rename(&r, s(&a[0]).unwrap_or_default(), s(&a[1]).ok_or_else(|| GitError::other("a branch needs a name"))?));
    m = repo_fn_cancellable(m, "branchDelete", 1, |r, a, c| {
        let o = a.get(1).cloned().unwrap_or(Value::Null);
        ops::branch_delete(&r, s(&a[0]).unwrap_or_default(), o.get("force").and_then(Value::as_bool).unwrap_or(false), o.get("remote").and_then(Value::as_bool).unwrap_or(false), c)
    });
    m = repo_fn(m, "branchTrack", |r, a| ops::branch_track(&r, s(&a[0]).unwrap_or_default(), a.get(1).and_then(s)));
    m = repo_fn(m, "compare", |r, a| ops::compare(&r, s(&a[0]).unwrap_or("HEAD"), s(&a[1]).unwrap_or("HEAD")));
    m = repo_fn(m, "merge", |r, a| ops::merge(&r, s(&a[0]).ok_or_else(|| GitError::other("merge what?"))?, a.get(1).unwrap_or(&Value::Null)));
    m = repo_fn(m, "rebase", |r, a| ops::rebase(&r, s(&a[0]).unwrap_or_default(), a.get(1).unwrap_or(&Value::Null)));
    m = repo_fn(m, "sequencer", |r, a| ops::sequencer(&r, s(&a[0]).unwrap_or_default()));
    m = repo_fn_cancellable(m, "fetch", 0, |r, a, c| ops::fetch(&r, a.first().unwrap_or(&Value::Null), c));
    m = repo_fn_cancellable(m, "pull", 0, |r, a, c| ops::pull(&r, a.first().unwrap_or(&Value::Null), c));
    m = repo_fn(m, "pushPreview", |r, _| ops::push_preview(&r));
    m = repo_fn_cancellable(m, "push", 0, |r, a, c| ops::push(&r, a.first().unwrap_or(&Value::Null), c));
    m = repo_fn(m, "remotes", |r, _| ops::remotes(&r));
    m = repo_fn(m, "remoteEdit", |r, a| ops::remote_edit(&r, s(&a[0]).unwrap_or_default(), s(&a[1]).unwrap_or_default(), a.get(2).and_then(s)));
    m = repo_fn(m, "stashes", |r, _| ops::stashes(&r));
    m = repo_fn(m, "stashPush", |r, a| ops::stash_push(&r, a.first().unwrap_or(&Value::Null)));
    m = repo_fn(m, "stashAction", |r, a| ops::stash_action(&r, s(&a[0]).unwrap_or_default(), a.get(1).and_then(s).unwrap_or("stash@{0}"), a.get(2).unwrap_or(&Value::Null)));
    m = repo_fn(m, "cherryPick", |r, a| ops::cherry_pick(&r, &strings(&a[0]), a.get(1).unwrap_or(&Value::Null)));
    m = repo_fn(m, "revert", |r, a| ops::revert(&r, &strings(&a[0]), a.get(1).unwrap_or(&Value::Null)));
    m = repo_fn(m, "reset", |r, a| ops::reset(&r, s(&a[0]).unwrap_or("HEAD"), s(&a[1]).unwrap_or("mixed")));
    m = repo_fn(m, "undoCommit", |r, _| ops::undo_commit(&r));
    m = repo_fn(m, "tagCreate", |r, a| ops::tag_create(&r, s(&a[0]).ok_or_else(|| GitError::other("a tag needs a name"))?, a.get(1).and_then(s), a.get(2).and_then(s)));
    m = repo_fn_cancellable(m, "tagDelete", 2, |r, a, c| ops::tag_delete(&r, s(&a[0]).unwrap_or_default(), a.get(1).and_then(s), c));
    m = repo_fn_cancellable(m, "tagPush", 2, |r, a, c| ops::tag_push(&r, s(&a[0]).unwrap_or("origin"), a.get(1).and_then(s), c));
    m = repo_fn(m, "patchCreate", |r, a| ops::patch_create(&r, a.first().unwrap_or(&Value::Null)).map(Value::String));
    m = repo_fn(m, "patchSave", |r, a| {
        let text = ops::patch_create(&r, a.first().unwrap_or(&Value::Null))?;
        let folder = s(a.get(1).unwrap_or(&Value::Null)).ok_or_else(|| GitError::other("patchSave needs a folder"))?;
        let name = s(a.get(2).unwrap_or(&Value::Null)).unwrap_or("changes.patch").replace(['/', '\\'], "-");
        let path = std::path::Path::new(folder).join(&name);
        std::fs::write(&path, text).map_err(|e| GitError::Other(format!("cannot write {}: {e}", path.display())))?;
        Ok(json!(path.to_string_lossy()))
    });
    m = repo_fn(m, "patchApply", |r, a| ops::patch_apply(&r, a.first().and_then(Value::as_str).unwrap_or_default(), a.get(1).unwrap_or(&Value::Null)));
    m = repo_fn(m, "worktrees", |r, _| ops::worktrees(&r));
    m = repo_fn(m, "worktreeAction", |r, a| ops::worktree_action(&r, s(&a[0]).unwrap_or_default(), a.get(1).and_then(s), a.get(2).and_then(Value::as_bool).unwrap_or(false)));
    m = repo_fn(m, "submodules", |r, _| ops::submodules(&r));
    m = repo_fn_cancellable(m, "submoduleUpdate", 0, |r, _, c| ops::submodule_update(&r, c));
    m = repo_fn(m, "config", |r, _| ops::config(&r));
    m = repo_fn(m, "setConfig", |r, a| ops::set_config(&r, s(&a[0]).unwrap_or_default(), a.get(1).and_then(Value::as_str), a.get(2).and_then(Value::as_bool).unwrap_or(false)));
    m = repo_fn(m, "blame", |r, a| ops::blame(&r, s(&a[0]).ok_or_else(|| GitError::other("blame needs a path"))?, a.get(1).and_then(s), a.get(2).unwrap_or(&Value::Null)).map(|v| (*v).clone()));
    m = repo_fn(m, "tree", |r, a| ops::tree(&r, s(&a[0]).unwrap_or("HEAD"), a.get(1).and_then(Value::as_str).unwrap_or("")));
    m = repo_fn(m, "fileAt", |r, a| {
        let spec = format!("{}:{}", s(&a[0]).unwrap_or("HEAD"), a.get(1).and_then(Value::as_str).unwrap_or_default());
        Ok(match diffview::blob(&r, &spec)? {
            Some(b) if b.len() <= 4 << 20 && !b[..b.len().min(8000)].contains(&0) => Value::String(String::from_utf8_lossy(&b).into_owned()),
            _ => Value::Null,
        })
    });
    m = repo_fn(m, "conflictOpen", |r, a| Ok(merge::keep(merge::ConflictSession::load(r, s(&a[0]).ok_or_else(|| GitError::other("which file?"))?)?).view()));
    m = m.async_function("conflictResolve", |a| {
        let c = merge::get(a.integer(0)? as u64).map_err(err)?;
        let choices = arg(a, 1);
        Ok(finish("resolve", move || c.resolve(&choices)))
    });
    m = repo_fn(m, "conflictAccept", |r, a| merge::accept(&r, &strings(&a[0]), s(a.get(1).unwrap_or(&Value::Null)).unwrap_or("ours")));
    m = repo_fn(m, "rebaseTodo", |r, a| rebase::todo_list(&r, s(&a[0]).ok_or_else(|| GitError::other("rebase from where?"))?));
    m = repo_fn(m, "rebaseInteractive", |r, a| {
        let entries = a.get(1).and_then(Value::as_array).cloned().unwrap_or_default().iter().map(rebase::Entry::from_json).collect::<GitResult<Vec<_>>>()?;
        let update_refs = a.get(2).and_then(|o| o.get("updateRefs")).and_then(Value::as_bool).unwrap_or(false);
        rebase::run(&r, s(&a[0]).ok_or_else(|| GitError::other("rebase from where?"))?, &entries, update_refs)
    });
    m = repo_fn(m, "rewordInMemory", |r, a| rebase::reword_in_memory(&r, s(&a[0]).unwrap_or_default(), a.get(1).and_then(Value::as_str).unwrap_or_default()));
    m
}

/// `from_host` re-exported for tests.
#[doc(hidden)]
pub fn json_of(v: &HostValue) -> Value {
    from_host(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuzora_core::HostArguments;

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:git");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
        assert!(m.is_async("status") && m.is_async("open") && !m.is_async("logRows"));
        let settings = json_of(&m.call("settings", &HostArguments::new([])).unwrap());
        assert_eq!(settings["forceWithLease"], true);
    }

    #[test]
    fn settings_refuse_a_pattern_the_engine_cant_read() {
        let m = host_module();
        // Look-ahead: JavaScript's RegExp takes it, the engine's regex doesn't.
        let ahead = "^(?!wip/).*";
        let check = |p: &str| m.call("checkPattern", &HostArguments::new([HostValue::Str(p.into())]));
        assert!(matches!(check(ahead), Ok(HostValue::Str(_))), "{:?}", check(ahead));
        assert_eq!(check("^(main|release/.*)$").unwrap(), HostValue::Null);
        let values = to_host(&json!({ "protectedBranches": ahead }));
        assert!(m.call("setSettings", &HostArguments::new([values])).is_err());
        assert_ne!(settings::text("protectedBranches"), ahead);
    }

    #[test]
    fn the_log_is_reachable_from_script_calls() {
        let s = crate::repo::testing::scratch();
        s.commit("a.txt", "1\n", "first");
        let r = repo::open(s.root(), None).unwrap();
        let m = host_module();
        let id = m.call("logOpen", &HostArguments::new([HostValue::Str(r.id.clone())])).unwrap();
        for _ in 0..300 {
            let info = json_of(&m.call("logInfo", &HostArguments::new([id.clone()])).unwrap());
            if info["loading"] == false && info["rows"] == 1 {
                let rows = json_of(&m.call("logRows", &HostArguments::new([id.clone(), HostValue::Number(0.0), HostValue::Number(5.0)])).unwrap());
                assert_eq!(rows[0]["subject"], "first");
                m.call("logClose", &HostArguments::new([id])).unwrap();
                repo::close(&r.id);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the log never loaded");
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_isnt_utf8_is_never_acted_on_by_how_it_is_shown() {
        use std::io::Write;
        let s = crate::repo::testing::scratch();
        s.commit("a.txt", "1\n", "first");
        // `café` in Latin-1, which status shows as `caf\u{FFFD}.txt`. Only the
        // index can hold it (APFS refuses the name), so it shows as deleted.
        let oid = s.git(&["rev-parse", "HEAD:a.txt"]).trim().to_owned();
        let mut add = std::process::Command::new("git").args(["update-index", "--index-info"]).current_dir(s.root()).stdin(std::process::Stdio::piped()).spawn().unwrap();
        add.stdin.take().unwrap().write_all(&[format!("100644 {oid}\t").as_bytes(), b"caf\xe9.txt\n"].concat()).unwrap();
        assert!(add.wait().unwrap().success());
        s.git(&["commit", "-qm", "latin-1"]);
        // And a file that really has the shown name.
        s.commit("caf\u{FFFD}.txt", "shown\n", "shown");
        s.write("caf\u{FFFD}.txt", "changed\n");
        let r = repo::open(s.root(), None).unwrap();
        let m = host_module();
        let id = HostValue::Str(r.id.clone());
        let call = |f: &str, rest: Value| crate::repo::tests::futures_lite_block(m.begin(f, &HostArguments::new([id.clone(), to_host(&rest)])).unwrap());
        let st = json_of(&call("status", json!({})).unwrap());
        assert!(st["unstaged"].as_array().unwrap().iter().any(|e| e["path"] == "caf\u{FFFD}.txt"), "{st}");
        for (f, args) in [("stage", json!(["caf\u{FFFD}.txt"])), ("discard", json!(["caf\u{FFFD}.txt"])), ("diffOpen", json!({ "path": "caf\u{FFFD}.txt" }))] {
            let e = call(f, args).unwrap_err();
            assert!(e.message().contains("caf\u{FFFD}.txt"), "{f}: {}", e.message());
        }
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), "");
        assert_eq!(std::fs::read_to_string(format!("{}/caf\u{FFFD}.txt", s.root())).unwrap(), "changed\n");
        // The file that really has the shown name changes: reading that name
        // again mustn't lose the other one.
        s.write("caf\u{FFFD}.txt", "again\n");
        r.invalidate(status::Scope::Paths(vec!["caf\u{FFFD}.txt".into()]));
        call("status", json!({})).unwrap();
        assert!(call("stage", json!(["caf\u{FFFD}.txt"])).is_err(), "after a partial refresh");
        // Names that are UTF-8 still work, and so does a folder above the odd one.
        call("stage", json!(["a.txt", "."])).unwrap();
        repo::close(&r.id);
    }
}
