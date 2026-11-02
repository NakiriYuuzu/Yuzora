//! Open documents: one buffer per file (machine + path) for every pane that
//! shows it, the editor that edits it, and the round trip to disk: loading,
//! saving with a conflict check, and noticing changes made by someone else.
//!
//! A document lives in the app (a GPUI global), so the `Editor` and
//! `Markdown` components and the `yuzora:files` functions all reach the same
//! one. Disk work runs on worker threads; anything that needs the window
//! (setting text, focus, replacing) is deferred to a foreground task, never
//! run inside the script call that asked for it.
//!
//! One pane at a time draws a document's editor (its *owner*); another pane
//! showing the same file offers to take it over.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::channel::oneshot;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_base::input::{EditorState, InputEvent, Position, Rope};
use yuzora_core::{HostObject, HostValue};

use crate::editor::{self, CodeEditorOptions, Look};
use crate::fsops::{self, Eol, Stamp, TextFile, WriteError};
use crate::highlight::{self, Lang};

/// Files above this are not opened at all.
pub const MAX_OPEN_BYTES: u64 = 16 << 20;

/// Which file a document holds. `on`: a machine from `hc machine ls`, `None` here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocKey {
    pub on: Option<String>,
    pub path: String,
}

#[derive(Default)]
struct Docs {
    by_id: HashMap<u64, Entity<DocView>>,
    by_key: HashMap<DocKey, u64>,
    next: u64,
}

impl Global for Docs {}

pub fn get(id: u64, cx: &App) -> Option<Entity<DocView>> {
    cx.try_global::<Docs>()?.by_id.get(&id).cloned()
}

/// The document for a file, opened (and loading) if nobody has it yet. Each
/// call holds one reference until [`release`].
pub fn open(key: DocKey, cx: &mut App) -> u64 {
    if let Some(id) = cx.default_global::<Docs>().by_key.get(&key).copied()
        && let Some(view) = get(id, cx)
    {
        view.update(cx, |doc, cx| {
            doc.refs += 1;
            // A retained, unsaved buffer comes back as it was; anything else
            // is checked against the disk.
            if doc.refs == 1 && !doc.dirty {
                drop(doc.check_disk(cx));
            }
        });
        return id;
    }
    let docs = cx.default_global::<Docs>();
    docs.next += 1;
    let id = docs.next;
    let view = cx.new(|cx| {
        let mut doc = DocView::new(id, key.clone());
        doc.load(cx);
        doc
    });
    let docs = cx.default_global::<Docs>();
    docs.by_id.insert(id, view);
    docs.by_key.insert(key, id);
    id
}

/// Drops one reference (and `owner`'s claim on the editor). A document nobody
/// holds is forgotten, unless it has unsaved changes: those stay in memory and
/// come back when the file is opened again.
pub fn release(id: u64, owner: Option<&str>, cx: &mut App) {
    let Some(view) = get(id, cx) else { return };
    let gone = view.update(cx, |doc, cx| {
        doc.refs = doc.refs.saturating_sub(1);
        if owner.is_some() && doc.owner.as_deref() == owner {
            doc.owner = None;
            cx.notify();
        }
        doc.refs == 0 && !doc.dirty
    });
    if gone {
        forget(id, cx);
    }
}

fn forget(id: u64, cx: &mut App) {
    let docs = cx.default_global::<Docs>();
    if let Some(view) = docs.by_id.remove(&id) {
        let key = view.read(cx).key.clone();
        let docs = cx.default_global::<Docs>();
        if docs.by_key.get(&key) == Some(&id) {
            docs.by_key.remove(&key);
        }
        view.update(cx, |doc, _| doc.waiters.clear());
    }
}

/// Renames a file or folder (this machine only), then points the documents
/// open at it, or under it, at the new path: their panes keep showing them,
/// and the next look at the disk finds the file where it now is.
pub fn rename(
    from: String,
    to: String,
    on: Option<String>,
    cx: &mut App,
) -> oneshot::Receiver<Result<(), String>> {
    let (tx, rx) = oneshot::channel();
    cx.spawn(async move |cx| {
        let (f, t, o) = (from.clone(), to.clone(), on);
        let result = blocking(move || fsops::rename(&f, &t, o.as_deref()))
            .await
            .unwrap_or_else(|| Err("could not start a worker".into()));
        if result.is_ok() {
            cx.update(|cx| moved(&from, &to, cx));
        }
        let _ = tx.send(result);
    })
    .detach();
    rx
}

/// Re-keys the documents on this machine at `from`, or under it, to `to`.
fn moved(from: &str, to: &str, cx: &mut App) {
    for view in all(cx) {
        let (id, key) = {
            let doc = view.read(cx);
            (doc.id, doc.key.clone())
        };
        let Some(path) = renamed(&key.path, from, to).filter(|_| key.on.is_none()) else {
            continue;
        };
        let new_key = DocKey { on: None, path };
        let docs = cx.default_global::<Docs>();
        if docs.by_key.get(&key) == Some(&id) {
            docs.by_key.remove(&key);
        }
        docs.by_key.insert(new_key.clone(), id);
        view.update(cx, |doc, cx| {
            doc.key = new_key;
            doc.bump(cx);
        });
    }
}

/// `path` after `from` moved to `to`, if it is `from` or inside it.
fn renamed(path: &str, from: &str, to: &str) -> Option<String> {
    if path == from {
        return Some(to.to_owned());
    }
    let rest = path.strip_prefix(from)?;
    rest.starts_with(['/', '\\']).then(|| format!("{to}{rest}"))
}

/// The document already open for a file, if any.
pub fn find(key: &DocKey, cx: &App) -> Option<u64> {
    cx.try_global::<Docs>()?.by_key.get(key).copied()
}

/// How many open documents have unsaved changes.
pub fn unsaved(cx: &App) -> usize {
    all(cx).iter().filter(|d| d.read(cx).dirty).count()
}

/// The document a herdr pane was last seen showing.
pub fn for_pane(pane: &str, cx: &App) -> Option<Entity<DocView>> {
    all(cx)
        .into_iter()
        .find(|d| d.read(cx).pane.as_deref() == Some(pane))
}

/// Every open document (for "which panes show this file").
pub fn all(cx: &App) -> Vec<Entity<DocView>> {
    cx.try_global::<Docs>()
        .map(|d| d.by_id.values().cloned().collect())
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Loading,
    Ready,
    Failed(String),
}

/// What reading the file found out about it.
#[derive(Clone, Debug)]
struct Facts {
    lang: Option<Lang>,
    eol: Eol,
    bom: bool,
    size: u64,
    binary: bool,
    too_large: bool,
    not_utf8: bool,
    readonly: bool,
    /// Too big to highlight: plain text, no folding.
    large: bool,
}

impl Facts {
    fn new(key: &DocKey) -> Self {
        Self {
            lang: Lang::detect(&key.path),
            eol: Eol::Lf,
            bom: false,
            size: 0,
            binary: false,
            too_large: false,
            not_utf8: false,
            readonly: key.on.is_some(),
            large: false,
        }
    }

    fn from_file(key: &DocKey, f: &TextFile) -> Self {
        Self {
            lang: Lang::detect(&key.path),
            eol: f.eol,
            bom: f.bom,
            size: f.size,
            binary: f.binary,
            too_large: f.too_large,
            not_utf8: f.not_utf8,
            readonly: f.readonly || f.not_utf8 || key.on.is_some(),
            large: f.size > highlight::MAX_HIGHLIGHT_BYTES as u64,
        }
    }

    /// Whether there is text to edit at all.
    fn editable_text(&self) -> bool {
        !self.binary && !self.too_large
    }
}

/// What a disk check found.
enum Probe {
    Same,
    Missing,
    /// Same content, new timestamp (a touch, or our own write seen late).
    Touched(Stamp),
    /// Same stamp, other permission bits: whether it can be written now.
    Access {
        mode: u32,
        readonly: bool,
    },
    Changed(TextFile),
    Failed(String),
}

/// How a save ended, for the script.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub ok: bool,
    pub conflict: bool,
    pub error: Option<String>,
}

impl Saved {
    fn error(e: impl Into<String>) -> Self {
        Self {
            ok: false,
            conflict: false,
            error: Some(e.into()),
        }
    }

    pub fn to_host(&self) -> HostValue {
        HostObject::new()
            .field("ok", self.ok)
            .field("conflict", self.conflict)
            .field("error", self.error.clone())
            .into()
    }
}

/// One open file.
pub struct DocView {
    pub id: u64,
    pub key: DocKey,
    phase: Phase,
    facts: Facts,
    /// Loaded before any pane drew the editor; becomes the editor's text.
    pending: Option<String>,
    state: Option<Entity<EditorState>>,
    window: Option<AnyWindowHandle>,
    /// The text as last loaded or saved: the editor is dirty when it differs.
    baseline: Rope,
    disk: Option<Stamp>,
    disk_hash: u64,
    /// The permission bits last seen ([`fsops::mode`]), here only.
    disk_mode: Option<u32>,
    /// What a check found on disk while there were unsaved changes.
    incoming: Option<TextFile>,
    dirty: bool,
    conflict: bool,
    deleted: bool,
    saving: bool,
    checking: bool,
    /// Text from disk is on its way into the editor: a save now would pair
    /// the old text with the new stamp.
    applying: bool,
    /// Bumped whenever the text changes (for the Markdown preview).
    rev: u64,
    /// When someone else last changed the file (ms since the epoch).
    external_at: Option<u64>,
    error: Option<String>,
    owner: Option<String>,
    refs: usize,
    focused: bool,
    /// The herdr pane a script last saw showing it.
    pane: Option<String>,
    seq: u64,
    waiters: Vec<oneshot::Sender<u64>>,
    look: Look,
    status_bar: bool,
    theme_name: SharedString,
    _subscriptions: Vec<Subscription>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs blocking work on its own thread and waits for it without blocking.
/// (Its own thread, not GPUI's background pool: a read on another machine can
/// take seconds.) Tests run it inline: GPUI's test scheduler refuses wakeups
/// from threads it does not drive.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if cfg!(test) {
        return Some(work());
    }
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("yuzora-files".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .ok()?;
    rx.await.ok()
}

/// The bytes `fsops::write_text` puts on disk, for the content hash.
fn encoded(text: &str, eol: Eol, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if bom {
        out.extend_from_slice(b"\xEF\xBB\xBF");
    }
    match eol {
        Eol::Lf => out.extend_from_slice(text.as_bytes()),
        Eol::Crlf => out.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    out
}

impl DocView {
    fn new(id: u64, key: DocKey) -> Self {
        let facts = Facts::new(&key);
        Self {
            id,
            key,
            phase: Phase::Loading,
            facts,
            pending: None,
            state: None,
            window: None,
            baseline: Rope::new(),
            disk: None,
            disk_hash: 0,
            disk_mode: None,
            incoming: None,
            dirty: false,
            conflict: false,
            deleted: false,
            saving: false,
            checking: false,
            applying: false,
            rev: 0,
            external_at: None,
            error: None,
            owner: None,
            refs: 1,
            focused: false,
            pane: None,
            seq: 1,
            waiters: Vec::new(),
            look: Look::default(),
            status_bar: true,
            theme_name: SharedString::new_static("One Dark"),
            _subscriptions: Vec::new(),
        }
    }

    /// Wakes every `documentChanged` waiter and redraws.
    fn bump(&mut self, cx: &mut Context<Self>) {
        self.seq += 1;
        for waiter in self.waiters.drain(..) {
            let _ = waiter.send(self.seq);
        }
        cx.notify();
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Resolves once something changed after `seen`.
    pub fn wait(&mut self, seen: u64) -> oneshot::Receiver<u64> {
        let (tx, rx) = oneshot::channel();
        if self.seq > seen {
            let _ = tx.send(self.seq);
        } else {
            self.waiters.push(tx);
        }
        rx
    }

    pub fn set_pane(&mut self, pane: Option<String>) {
        self.pane = pane;
    }

    pub fn pane(&self) -> Option<&str> {
        self.pane.as_deref()
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// The text now: the editor's, else what was loaded.
    pub fn text(&self, cx: &App) -> SharedString {
        match &self.state {
            Some(state) => state.read(cx).value(),
            None => self
                .pending
                .clone()
                .map(SharedString::from)
                .unwrap_or_else(|| self.baseline.to_string().into()),
        }
    }

    pub fn editor(&self) -> Option<&Entity<EditorState>> {
        self.state.as_ref()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Loading;
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let (read, gone) = blocking(move || {
                let read = fsops::read_text(&key.path, key.on.as_deref(), MAX_OPEN_BYTES);
                // Gone only when a stat says so, not when it is unreadable or out of reach.
                let gone =
                    read.is_err() && matches!(fsops::stat(&key.path, key.on.as_deref()), Ok(None));
                (read, gone)
            })
            .await
            .unwrap_or_else(|| (Err("could not start a worker".into()), false));
            let _ = this.update(cx, |doc, cx| doc.loaded(read, gone, cx));
        })
        .detach();
    }

    /// `gone`: a failed read found no file there.
    fn loaded(&mut self, read: Result<TextFile, String>, gone: bool, cx: &mut Context<Self>) {
        match read {
            // A re-read that failed keeps the text there is: it may be all
            // that is left of a file removed from disk.
            Err(e) if self.state.is_some() => {
                self.phase = Phase::Ready;
                self.deleted = gone;
                self.conflict = false;
                self.error = Some(e);
            }
            Err(e) => {
                self.phase = Phase::Failed(e);
            }
            Ok(file) => {
                self.facts = Facts::from_file(&self.key, &file);
                self.disk = Some(file.stamp);
                self.disk_hash = file.hash;
                self.deleted = false;
                self.conflict = false;
                self.incoming = None;
                self.error = None;
                self.phase = Phase::Ready;
                if self.facts.editable_text() {
                    self.baseline = Rope::from(file.text.as_str());
                    self.set_text(file.text, cx);
                }
                self.sync_readonly(cx);
            }
        }
        self.bump(cx);
    }

    /// The editor follows the file's read-only state as it changes.
    fn sync_readonly(&self, cx: &mut Context<Self>) {
        if let Some(state) = &self.state {
            let readonly = self.facts.readonly;
            state.update(cx, |s, cx| s.set_readonly(readonly, cx));
        }
    }

    /// Puts `text` in the editor (keeping the cursor and scroll), or keeps it
    /// for the editor not drawn yet.
    fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.rev += 1;
        let (Some(state), Some(window)) = (self.state.clone(), self.window) else {
            self.pending = Some(text);
            self.dirty = false;
            return;
        };
        self.applying = true;
        cx.spawn(async move |this, cx| {
            let _ = window.update(cx, |_, window, cx| {
                state.update(cx, |s, cx| {
                    let cursor = s.cursor();
                    let scroll = s.scroll_offset();
                    s.set_value(text, window, cx);
                    let at = cursor.min(s.text().len());
                    s.set_selected_range(at..at, cx);
                    s.set_scroll_offset(scroll, cx);
                });
            });
            let _ = this.update(cx, |doc, cx| {
                doc.applying = false;
                doc.rev += 1;
                doc.dirty = doc.compute_dirty(cx);
                doc.bump(cx);
            });
        })
        .detach();
    }

    fn compute_dirty(&self, cx: &App) -> bool {
        match &self.state {
            Some(state) => {
                let text = state.read(cx).text();
                text.len() != self.baseline.len() || *text != self.baseline
            }
            None => false,
        }
    }

    fn on_input(&mut self, _: Entity<EditorState>, event: &InputEvent, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                self.rev += 1;
                let dirty = self.compute_dirty(cx);
                if dirty != self.dirty {
                    self.dirty = dirty;
                    self.bump(cx);
                }
            }
            InputEvent::Focus => {
                self.focused = true;
                self.bump(cx);
            }
            InputEvent::Blur => {
                self.focused = false;
                self.bump(cx);
            }
            _ => {}
        }
    }

    /// Before an owner draws: builds the editor once the text is there.
    fn prepare(
        &mut self,
        look: Look,
        status_bar: bool,
        theme: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.look != look {
            if let Some(state) = &self.state {
                let style = look.editor_style();
                state.update(cx, |s, _| s.set_editor_style(style));
            }
            self.look = look;
        }
        self.status_bar = status_bar;
        self.theme_name = theme;
        self.window = Some(window.window_handle());
        if self.state.is_some() || self.phase != Phase::Ready || !self.facts.editable_text() {
            return;
        }
        let text = self
            .pending
            .take()
            .unwrap_or_else(|| self.baseline.to_string());
        let large = self.facts.large;
        let state = editor::new_code_editor(
            CodeEditorOptions {
                lang: if large { None } else { self.facts.lang },
                text: &text,
                readonly: self.facts.readonly,
                folding: !large,
                ..CodeEditorOptions::default()
            },
            &self.look,
            window,
            cx,
        );
        self._subscriptions
            .push(cx.subscribe(&state, Self::on_input));
        self._subscriptions
            .push(cx.observe(&state, |_, _, cx| cx.notify()));
        self.state = Some(state);
        self.dirty = self.compute_dirty(cx);
        // Scripts waiting on this (a line to go to, focus on open) can act now.
        self.bump(cx);
    }

    /// Writes the editor's text. `force`: even if the file changed on disk.
    pub fn save(&mut self, force: bool, cx: &mut Context<Self>) -> oneshot::Receiver<Saved> {
        let (tx, rx) = oneshot::channel();
        let Some(state) = self.state.clone() else {
            let _ = tx.send(Saved::error("not loaded"));
            return rx;
        };
        if self.key.on.is_some() {
            let _ = tx.send(Saved::error("remote"));
            return rx;
        }
        if self.facts.readonly {
            let _ = tx.send(Saved::error("readonly"));
            return rx;
        }
        if self.saving || self.applying {
            let _ = tx.send(Saved::error("busy"));
            return rx;
        }
        let rope = state.read(cx).text().clone();
        let text = rope.to_string();
        let expected = if force {
            None
        } else {
            self.disk.map(|stamp| (stamp, self.disk_hash))
        };
        let (path, eol, bom) = (self.key.path.clone(), self.facts.eol, self.facts.bom);
        self.saving = true;
        self.bump(cx);
        cx.spawn(async move |this, cx| {
            let hash = fsops::content_hash(&encoded(&text, eol, bom));
            let result = blocking(move || fsops::write_text(&path, &text, eol, bom, expected))
                .await
                .unwrap_or_else(|| Err(WriteError::Io("could not start a worker".into())));
            let _ = this.update(cx, |doc, cx| {
                doc.saving = false;
                let saved = match result {
                    Ok(stamp) => {
                        doc.disk = Some(stamp);
                        doc.disk_hash = hash;
                        doc.facts.size = stamp.len;
                        doc.baseline = rope;
                        doc.conflict = false;
                        doc.deleted = false;
                        doc.incoming = None;
                        doc.dirty = doc.compute_dirty(cx);
                        Saved {
                            ok: true,
                            conflict: false,
                            error: None,
                        }
                    }
                    Err(WriteError::Conflict(now)) => {
                        // What is there now: a conflict to resolve, or a file gone.
                        // Read even under the same stamp: the bytes may be what changed.
                        doc.conflict = now.is_some();
                        doc.deleted = now.is_none();
                        drop(doc.look_at_disk(true, cx));
                        Saved {
                            ok: false,
                            conflict: true,
                            error: None,
                        }
                    }
                    Err(WriteError::Remote) => Saved::error("remote"),
                    Err(WriteError::Io(e)) => Saved::error(e),
                };
                doc.bump(cx);
                let _ = tx.send(saved);
                // Saved after its last pane closed: nothing holds it any more.
                if doc.refs == 0 && !doc.dirty {
                    let id = doc.id;
                    cx.defer(move |cx| forget(id, cx));
                }
            });
        })
        .detach();
        rx
    }

    /// Reads the file again, dropping unsaved changes.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.incoming = None;
        self.load(cx);
        self.bump(cx);
    }

    /// Keeps the unsaved changes over what changed on disk: the next save
    /// overwrites it without asking again.
    pub fn keep_mine(&mut self, cx: &mut Context<Self>) {
        if let Some(file) = self.incoming.take() {
            self.disk = Some(file.stamp);
            self.disk_hash = file.hash;
        }
        self.conflict = false;
        self.bump(cx);
    }

    /// Looks at the file on disk: a clean document follows a change made
    /// elsewhere; a dirty one is marked as in conflict.
    pub fn check_disk(&mut self, cx: &mut Context<Self>) -> oneshot::Receiver<()> {
        self.look_at_disk(false, cx)
    }

    /// [`Self::check_disk`]; `read`: compare the content even when the stamp
    /// is the same (a save was refused for its bytes).
    fn look_at_disk(&mut self, read: bool, cx: &mut Context<Self>) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        if self.checking || self.saving || self.phase == Phase::Loading {
            let _ = tx.send(());
            return rx;
        }
        if matches!(self.phase, Phase::Failed(_)) {
            // It may exist now.
            self.load(cx);
            let _ = tx.send(());
            return rx;
        }
        self.checking = true;
        // While in conflict, "unchanged" means unchanged since the conflict was seen.
        let (disk, disk_hash) = match &self.incoming {
            Some(file) => (Some(file.stamp), file.hash),
            None => (self.disk, self.disk_hash),
        };
        let key = self.key.clone();
        let base = self.disk;
        let mode = self.disk_mode;
        cx.spawn(async move |this, cx| {
            let probe = blocking(move || match fsops::stat(&key.path, key.on.as_deref()) {
                Ok(None) => Probe::Missing,
                // Unchanged, unless only its permissions were (a chmod moves
                // neither size nor time).
                Ok(Some(stamp)) if Some(stamp) == disk && !read => {
                    match key.on.is_none().then(|| fsops::mode(&key.path)).flatten() {
                        Some(now) if Some(now) != mode => Probe::Access {
                            mode: now,
                            readonly: fsops::readonly(&key.path),
                        },
                        _ => Probe::Same,
                    }
                }
                Ok(Some(_)) => match fsops::read_text(&key.path, key.on.as_deref(), MAX_OPEN_BYTES)
                {
                    Ok(file) if file.hash == disk_hash => Probe::Touched(file.stamp),
                    Ok(file) => Probe::Changed(file),
                    Err(e) => Probe::Failed(e),
                },
                Err(e) => Probe::Failed(e),
            })
            .await
            .unwrap_or(Probe::Same);
            let _ = this.update(cx, |doc, cx| {
                doc.checking = false;
                // A save landed meanwhile: what was read is older than that.
                if doc.disk == base {
                    doc.probed(probe, cx);
                }
            });
            let _ = tx.send(());
        })
        .detach();
        rx
    }

    fn probed(&mut self, probe: Probe, cx: &mut Context<Self>) {
        match probe {
            Probe::Same => {}
            Probe::Failed(e) => {
                if self.error.as_deref() != Some(e.as_str()) {
                    self.error = Some(e);
                    self.bump(cx);
                }
            }
            Probe::Missing => {
                if !self.deleted {
                    self.deleted = true;
                    self.external_at = Some(now_ms());
                    self.bump(cx);
                }
            }
            Probe::Access { mode, readonly } => {
                self.disk_mode = Some(mode);
                let readonly = readonly || self.facts.not_utf8 || self.key.on.is_some();
                if readonly != self.facts.readonly {
                    self.facts.readonly = readonly;
                    self.sync_readonly(cx);
                    self.bump(cx);
                }
            }
            Probe::Touched(stamp) => {
                match &mut self.incoming {
                    Some(file) => file.stamp = stamp,
                    None => self.disk = Some(stamp),
                }
                if self.deleted {
                    self.deleted = false;
                    self.bump(cx);
                }
            }
            Probe::Changed(file) => {
                self.external_at = Some(now_ms());
                self.deleted = false;
                self.error = None;
                if self.dirty {
                    self.conflict = true;
                    self.incoming = Some(file);
                } else if !file.binary && !file.too_large && !self.facts.editable_text() {
                    // It became text: start over.
                    self.conflict = false;
                    self.incoming = None;
                    self.load(cx);
                } else {
                    self.conflict = false;
                    self.incoming = None;
                    self.facts = Facts::from_file(&self.key, &file);
                    self.disk = Some(file.stamp);
                    self.disk_hash = file.hash;
                    if self.facts.editable_text() {
                        self.baseline = Rope::from(file.text.as_str());
                        self.set_text(file.text, cx);
                    }
                    self.sync_readonly(cx);
                }
                self.bump(cx);
            }
        }
    }

    /// The script's view of this document.
    pub fn snapshot(&self, cx: &App) -> HostValue {
        let (line, column) = self
            .state
            .as_ref()
            .map(|s| {
                let p = s.read(cx).cursor_position();
                (p.line as u64 + 1, p.character as u64 + 1)
            })
            .unwrap_or((1, 1));
        let (matches, current) = self
            .state
            .as_ref()
            .map(|s| {
                let m = &s.read(cx).search_session().matcher;
                (m.len(), m.current())
            })
            .unwrap_or((0, None));
        let lang = self.facts.lang.filter(|_| !self.facts.large);
        let phase = match &self.phase {
            Phase::Loading => "loading",
            Phase::Ready => "ready",
            Phase::Failed(_) => "failed",
        };
        let path = &self.key.path;
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
        let dir = path[..path.len() - name.len()]
            .trim_end_matches(['/', '\\'])
            .to_owned();
        HostObject::new()
            .field("doc", self.id)
            .field("seq", self.seq)
            .field("path", path.clone())
            .field("on", self.key.on.clone())
            .field("name", name)
            .field("dir", dir)
            .field("phase", phase)
            .field(
                "error",
                match &self.phase {
                    Phase::Failed(e) => Some(e.clone()),
                    _ => self.error.clone(),
                },
            )
            .field("language", lang.map(Lang::id))
            .field("languageName", lang.map(Lang::name))
            .field("badge", self.facts.lang.map(Lang::badge))
            .field(
                "badgeColor",
                self.facts.lang.map(|l| format!("#{:06x}", l.badge_color())),
            )
            .field(
                "eol",
                match self.facts.eol {
                    Eol::Lf => "LF",
                    Eol::Crlf => "CRLF",
                },
            )
            .field("bom", self.facts.bom)
            .field("size", self.facts.size)
            .field("binary", self.facts.binary)
            .field("tooLarge", self.facts.too_large)
            .field("large", self.facts.large)
            .field("notUtf8", self.facts.not_utf8)
            .field("readonly", self.facts.readonly)
            .field("remote", self.key.on.is_some())
            .field("dirty", self.dirty)
            .field("saving", self.saving)
            .field("conflict", self.conflict)
            .field("deleted", self.deleted)
            .field("externalAt", self.external_at)
            .field("focused", self.focused)
            .field("owner", self.owner.clone())
            .field("pane", self.pane.clone())
            .field("line", line)
            .field("column", column)
            .field("matches", matches)
            .field("match", current.map(|c| c + 1))
            .into()
    }

    // Search: gpui-base's engine, driven by the script's own find bar.

    pub fn find(&mut self, query: &str, case_sensitive: bool, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let query = query.to_owned();
        state.update(cx, |s, cx| {
            if query.is_empty() {
                s.close_search(cx);
            } else {
                // The first match (from the top) is the current one already.
                s.set_search_query(query, !case_sensitive, cx);
            }
        });
        self.bump(cx);
    }

    pub fn find_step(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        state.update(cx, |s, cx| {
            if backwards {
                s.previous_search_match(cx);
            } else {
                s.next_search_match(cx);
            }
        });
        self.bump(cx);
    }

    pub fn end_find(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.state.clone() {
            state.update(cx, |s, cx| s.close_search(cx));
        }
        self.bump(cx);
    }

    /// Runs `body` with the editor and its window, after the current call.
    fn with_window(
        &mut self,
        cx: &mut Context<Self>,
        body: impl FnOnce(&mut EditorState, &mut Window, &mut Context<EditorState>) + 'static,
    ) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        let (Some(state), Some(window)) = (self.state.clone(), self.window) else {
            let _ = tx.send(());
            return rx;
        };
        cx.spawn(async move |this, cx| {
            let _ = window.update(cx, |_, window, cx| {
                state.update(cx, |s, cx| body(s, window, cx))
            });
            let _ = this.update(cx, |doc, cx| doc.bump(cx));
            let _ = tx.send(());
        })
        .detach();
        rx
    }

    pub fn replace_one(
        &mut self,
        replacement: String,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<()> {
        self.with_window(cx, move |s, window, cx| {
            s.replace_current_search_match(&replacement, window, cx);
        })
    }

    pub fn replace_all(
        &mut self,
        replacement: String,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<()> {
        self.with_window(cx, move |s, window, cx| {
            s.replace_all_search_matches(&replacement, window, cx);
        })
    }

    pub fn focus(&mut self, cx: &mut Context<Self>) {
        drop(self.with_window(cx, |s, window, cx| s.focus(window, cx)));
    }

    /// Puts the cursor on `line` (1-based), `column` (1-based), and focuses.
    pub fn goto(&mut self, line: u32, column: u32, cx: &mut Context<Self>) {
        drop(self.with_window(cx, move |s, window, cx| {
            s.set_cursor_position(
                Position::new(line.saturating_sub(1), column.saturating_sub(1)),
                window,
                cx,
            );
            s.focus(window, cx);
        }));
    }

    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let s = editor_strings();
        let (line, column) = self
            .state
            .as_ref()
            .map(|st| {
                let p = st.read(cx).cursor_position();
                (p.line + 1, p.character + 1)
            })
            .unwrap_or((1, 1));
        let lang = match self.facts.lang {
            Some(_) if self.facts.large => s.t("status.large", &[]),
            Some(l) => l.name().to_owned(),
            None => s.t("status.plain", &[]),
        };
        let encoding = format!(
            "UTF-8{} · {}",
            if self.facts.bom { " BOM" } else { "" },
            match self.facts.eol {
                Eol::Lf => "LF",
                Eol::Crlf => "CRLF",
            }
        );
        let muted = self
            .look
            .status
            .or(self.look.muted)
            .unwrap_or_else(|| gpui::rgb(0xa1a1aa).into());
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(24.))
            .px(px(10.))
            .gap(px(12.))
            .text_size(px(11.))
            .text_color(muted)
            .when_some(self.look.border, |el, c| el.border_t_1().border_color(c))
            .child(s.t(
                "status.position",
                &[("line", line.to_string()), ("column", column.to_string())],
            ))
            .child(lang)
            .child(encoding)
            .when(self.facts.readonly, |el| {
                el.child(s.t("status.readonly", &[]))
            })
            .child(div().flex_1())
            .child(self.theme_name.clone())
    }
}

impl Render for DocView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(state) = self.state.clone() else {
            return div().size_full().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(editor::code_editor(
                ("yuzora-editor", self.id as usize),
                &state,
                &self.look,
                cx,
            ))
            .when(self.status_bar, |el| el.child(self.status_bar(cx)))
            .into_any_element()
    }
}

fn editor_strings() -> &'static yuzora_core::i18n::Catalog {
    crate::strings()
}

/// The `Editor` component: `Editor.new(id, {doc, owner, status?, theme?, …look})`.
pub fn editor_component(
    args: gpui_shell::ComponentArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let props = args.props();
    let Some(view) = props
        .get("doc")
        .and_then(HostValue::as_number)
        .and_then(|id| get(id as u64, cx))
    else {
        return div().size_full().into_any_element();
    };
    let owner = props
        .get("owner")
        .and_then(HostValue::as_str)
        .unwrap_or(args.id())
        .to_owned();
    let status_bar = props
        .get("status")
        .and_then(HostValue::as_bool)
        .unwrap_or(true);
    let theme: SharedString = props
        .get("theme")
        .and_then(HostValue::as_str)
        .map(|t| SharedString::from(t.to_owned()))
        .unwrap_or_else(|| SharedString::new_static("One Dark"));
    let previous = view.read(cx).look.clone();
    let look = Look::from_props(props, Some(&previous));
    let owns = view.update(cx, |doc, cx| {
        if doc.owner.is_none() {
            doc.owner = Some(owner.clone());
        }
        let owns = doc.owner.as_deref() == Some(owner.as_str());
        if owns {
            doc.prepare(look.clone(), status_bar, theme, window, cx);
        }
        owns
    });
    if owns {
        return view.into_any_element();
    }
    elsewhere(view, owner, &look)
}

/// Drawn by a pane whose document another pane is editing.
fn elsewhere(view: Entity<DocView>, owner: String, look: &Look) -> AnyElement {
    let s = editor_strings();
    let muted = look
        .status
        .or(look.muted)
        .unwrap_or_else(|| gpui::rgb(0xa1a1aa).into());
    let border = look.border.unwrap_or_else(|| gpui::rgb(0x34353a).into());
    div()
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .text_size(px(12.))
        .text_color(muted)
        .child(s.t("elsewhere.title", &[]))
        .child(
            div()
                .id(SharedString::from(format!("yuzora-editor-claim-{owner}")))
                .px(px(10.))
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .cursor_pointer()
                .child(s.t("elsewhere.claim", &[]))
                .on_click(move |_, _, cx| {
                    view.update(cx, |doc, cx| {
                        doc.owner = Some(owner.clone());
                        doc.bump(cx);
                    });
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_matches_what_write_text_puts_on_disk() {
        assert_eq!(encoded("a\nb", Eol::Lf, false), b"a\nb");
        assert_eq!(encoded("a\nb", Eol::Crlf, true), b"\xEF\xBB\xBFa\r\nb");
    }

    #[test]
    fn facts_follow_the_file() {
        let key = DocKey {
            on: None,
            path: "/x/a.rs".into(),
        };
        let facts = Facts::new(&key);
        assert_eq!(facts.lang, Some(Lang::Rust));
        assert!(!facts.readonly);
        let remote = DocKey {
            on: Some("lab".into()),
            path: "/x/a.md".into(),
        };
        assert!(Facts::new(&remote).readonly);
    }

    /// A document with its editor drawn in a test window.
    fn editing(
        path: &std::path::Path,
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<DocView>, gpui::AnyWindowHandle) {
        let key = DocKey {
            on: None,
            path: path.to_string_lossy().into_owned(),
        };
        let id = cx.update(|cx| open(key, cx));
        cx.run_until_parked();
        let view = cx.update(|cx| get(id, cx).unwrap());
        let window = cx.add_window(|_, _| gpui::Empty);
        let handle: gpui::AnyWindowHandle = window.into();
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |doc, cx| {
                    doc.prepare(Look::default(), true, "One Dark".into(), window, cx)
                })
            })
            .unwrap();
        cx.run_until_parked();
        (view, handle)
    }

    fn type_text(
        view: &Entity<DocView>,
        window: gpui::AnyWindowHandle,
        text: &str,
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::EntityInputHandler as _;
        let state = cx.update(|cx| view.read(cx).state.clone().unwrap());
        cx.update_window(window, |_, window, cx| {
            state.update(cx, |s, cx| s.replace_text_in_range(None, text, window, cx))
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn unsaved_changes_meet_a_change_on_disk(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (view, window) = editing(&path, cx);
        type_text(&view, window, "mine ", cx);
        cx.update(|cx| assert!(view.read(cx).dirty));
        // Someone else writes the file: a conflict, the edit kept.
        std::fs::write(&path, "theirs, longer\n").unwrap();
        drop(view.update(cx, |doc, cx| doc.check_disk(cx)));
        cx.run_until_parked();
        cx.update(|cx| {
            let doc = view.read(cx);
            assert!(doc.conflict && doc.dirty && doc.incoming.is_some());
            assert_eq!(doc.text(cx).as_ref(), "mine one\n");
        });
        // Another look finds nothing new: no re-read, no fresh "changed elsewhere".
        let seen = cx.update(|cx| (view.read(cx).seq, view.read(cx).external_at));
        drop(view.update(cx, |doc, cx| doc.check_disk(cx)));
        cx.run_until_parked();
        cx.update(|cx| assert_eq!((view.read(cx).seq, view.read(cx).external_at), seen));
        // Keep mine, then save: written without a second conflict.
        view.update(cx, |doc, cx| doc.keep_mine(cx));
        drop(view.update(cx, |doc, cx| doc.save(false, cx)));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine one\n");
        cx.update(|cx| assert!(!view.read(cx).dirty && !view.read(cx).conflict));
    }

    #[gpui::test]
    fn a_save_that_meets_a_change_shows_what_is_there(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (view, window) = editing(&path, cx);
        type_text(&view, window, "mine ", cx);
        // Changed on disk before any look: the save finds out, and the conflict has their text.
        std::fs::write(&path, "theirs, longer\n").unwrap();
        drop(view.update(cx, |doc, cx| doc.save(false, cx)));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs, longer\n");
        cx.update(|cx| assert!(view.read(cx).conflict && view.read(cx).incoming.is_some()));
        // Gone from disk: a save finds it deleted, and dropping the edit keeps the text.
        std::fs::remove_file(&path).unwrap();
        view.update(cx, |doc, cx| doc.keep_mine(cx));
        drop(view.update(cx, |doc, cx| doc.save(false, cx)));
        cx.run_until_parked();
        cx.update(|cx| assert!(view.read(cx).deleted && !view.read(cx).conflict));
        view.update(cx, |doc, cx| doc.reload(cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let doc = view.read(cx);
            assert_eq!(doc.phase, Phase::Ready);
            assert!(doc.deleted && !doc.conflict);
            assert_eq!(doc.text(cx).as_ref(), "mine one\n");
        });
        // Saving again puts it back.
        drop(view.update(cx, |doc, cx| doc.save(true, cx)));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine one\n");
        cx.update(|cx| assert!(!view.read(cx).deleted && !view.read(cx).dirty));
    }

    #[test]
    fn renames_reach_the_paths_inside() {
        assert_eq!(
            renamed("/r/a.txt", "/r/a.txt", "/r/b.txt").as_deref(),
            Some("/r/b.txt")
        );
        assert_eq!(
            renamed("/r/d/a.txt", "/r/d", "/r/e").as_deref(),
            Some("/r/e/a.txt")
        );
        assert_eq!(renamed("/r/dd/a.txt", "/r/d", "/r/e"), None);
        assert_eq!(renamed("/r/a.txt", "/r/b.txt", "/r/c.txt"), None);
    }

    #[gpui::test]
    fn an_open_document_follows_its_file_when_renamed(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), "one\n").unwrap();
        let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let old = DocKey {
            on: None,
            path: path(&sub.join("a.txt")),
        };
        let id = cx.update(|cx| open(old.clone(), cx));
        cx.run_until_parked();
        // The file, then its folder.
        let renamed_to =
            |from: &std::path::Path, to: &std::path::Path, cx: &mut gpui::TestAppContext| {
                let rx = cx.update(|cx| rename(path(from), path(to), None, cx));
                cx.run_until_parked();
                assert_eq!(futures::executor::block_on(rx).unwrap(), Ok(()));
            };
        renamed_to(&sub.join("a.txt"), &sub.join("b.txt"), cx);
        let moved = DocKey {
            on: None,
            path: path(&dir.path().join("sub2/b.txt")),
        };
        renamed_to(&sub, &dir.path().join("sub2"), cx);
        cx.update(|cx| {
            assert_eq!(find(&old, cx), None);
            assert_eq!(find(&moved, cx), Some(id));
            assert_eq!(get(id, cx).unwrap().read(cx).key, moved);
        });
        // Where it is now, it is not deleted.
        let view = cx.update(|cx| get(id, cx).unwrap());
        drop(view.update(cx, |doc, cx| doc.check_disk(cx)));
        cx.run_until_parked();
        cx.update(|cx| assert!(!view.read(cx).deleted));
    }

    #[cfg(unix)]
    #[gpui::test]
    fn the_editor_follows_a_change_of_permissions(cx: &mut gpui::TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (view, _window) = editing(&path, cx);
        let check = |cx: &mut gpui::TestAppContext| {
            drop(view.update(cx, |doc, cx| doc.check_disk(cx)));
            cx.run_until_parked();
            cx.update(|cx| {
                let doc = view.read(cx);
                let editor = !doc.state.as_ref().unwrap().read(cx).is_editable();
                assert_eq!(editor, doc.facts.readonly);
                doc.facts.readonly
            })
        };
        assert!(!check(cx));
        // chmod moves neither the size nor the time.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(check(cx));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!check(cx));
    }

    #[gpui::test]
    fn a_save_updates_the_size(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (view, window) = editing(&path, cx);
        type_text(&view, window, "longer ", cx);
        drop(view.update(cx, |doc, cx| doc.save(false, cx)));
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).facts.size, "longer one\n".len() as u64));
    }

    #[cfg(unix)]
    #[gpui::test]
    fn an_unreadable_file_is_not_taken_for_a_deleted_one(cx: &mut gpui::TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let path = sub.join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (view, _window) = editing(&path, cx);
        // Its folder shut: there, but neither readable nor stat-able.
        let mode = |m: u32| std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(m));
        mode(0o000).unwrap();
        if std::fs::metadata(&path).is_ok() {
            // Root sees it anyway.
            mode(0o755).unwrap();
            return;
        }
        view.update(cx, |doc, cx| doc.reload(cx));
        cx.run_until_parked();
        mode(0o755).unwrap();
        cx.update(|cx| {
            let doc = view.read(cx);
            assert!(!doc.deleted && doc.error.is_some());
            assert_eq!(doc.text(cx).as_ref(), "one\n");
        });
        // Removed: that is deleted.
        std::fs::remove_file(&path).unwrap();
        view.update(cx, |doc, cx| doc.reload(cx));
        cx.run_until_parked();
        cx.update(|cx| assert!(view.read(cx).deleted));
    }

    #[gpui::test]
    fn keep_mine_resolves_a_change_the_stamp_did_not_show(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (view, window) = editing(&path, cx);
        type_text(&view, window, "mine ", cx);
        // Same length, and the clock put back: only the bytes tell.
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "two\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(before)
            .unwrap();
        drop(view.update(cx, |doc, cx| doc.save(false, cx)));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        cx.update(|cx| {
            let doc = view.read(cx);
            assert!(doc.conflict);
            assert_eq!(
                doc.incoming.as_ref().map(|f| f.text.as_str()),
                Some("two\n")
            );
        });
        // Keeping mine takes what is there as the base, so the next save writes.
        view.update(cx, |doc, cx| doc.keep_mine(cx));
        drop(view.update(cx, |doc, cx| doc.save(false, cx)));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine one\n");
        cx.update(|cx| assert!(!view.read(cx).conflict && !view.read(cx).dirty));
    }

    #[gpui::test]
    fn a_document_loads_follows_the_disk_and_is_shared(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\r\ntwo\r\n").unwrap();
        let key = DocKey {
            on: None,
            path: path.to_string_lossy().into_owned(),
        };
        let id = cx.update(|cx| open(key.clone(), cx));
        cx.run_until_parked();
        let view = cx.update(|cx| get(id, cx).unwrap());
        cx.update(|cx| {
            let doc = view.read(cx);
            assert_eq!(doc.phase, Phase::Ready);
            assert_eq!(doc.text(cx).as_ref(), "one\ntwo\n");
            assert_eq!(doc.facts.eol, Eol::Crlf);
            assert!(!doc.dirty && !doc.deleted && doc.external_at.is_none());
        });
        // Changed on disk with nothing unsaved: taken as it is, and remembered as external.
        std::fs::write(&path, "three, and longer\n").unwrap();
        drop(view.update(cx, |doc, cx| doc.check_disk(cx)));
        cx.run_until_parked();
        cx.update(|cx| {
            let doc = view.read(cx);
            assert_eq!(doc.text(cx).as_ref(), "three, and longer\n");
            assert_eq!(doc.facts.eol, Eol::Lf);
            assert!(!doc.conflict && doc.external_at.is_some());
        });
        // Gone from disk: flagged, the text kept.
        std::fs::remove_file(&path).unwrap();
        drop(view.update(cx, |doc, cx| doc.check_disk(cx)));
        cx.run_until_parked();
        cx.update(|cx| assert!(view.read(cx).deleted));
        cx.update(|cx| assert_eq!(view.read(cx).text(cx).as_ref(), "three, and longer\n"));
        // A second open shares it; releasing both forgets it.
        assert_eq!(cx.update(|cx| open(key.clone(), cx)), id);
        assert_eq!(cx.update(|cx| all(cx).len()), 1);
        cx.update(|cx| release(id, None, cx));
        assert!(cx.update(|cx| get(id, cx)).is_some());
        cx.update(|cx| release(id, None, cx));
        assert!(cx.update(|cx| get(id, cx)).is_none());
    }
}
