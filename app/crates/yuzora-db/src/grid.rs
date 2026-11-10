//! `DataGrid`: one result's rows as a native element.
//!
//! - Only the rows on screen are built (`uniform_list`), so 100k rows scroll
//!   like 10; rows that arrive while the query streams show up as they come.
//! - Header: click sorts (ascending, descending, back to the server's order);
//!   drag a header's right edge to resize the column.
//! - Cells: click selects, shift-click / shift-arrows extend, ⌘C (Ctrl+C)
//!   copies the selection as tab-separated text.
//! - Editing (a table's rows with a primary key): double-click or Enter edits
//!   in place, Enter keeps it, Esc drops it, ⌘⌫ sets NULL. Pending edits show
//!   in the footer with Discard and Write back (one transaction).
//! - Footer: `2,104 rows · showing 1–8 · 18 ms`.
//!
//! The grid keeps no rows of its own: it reads them from [`results`] while it
//! draws. Colors come from the script (`props.colors`, theme names), so the
//! grid follows the person's theme.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use gpui::EntityInputHandler as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render,
    ScrollWheelEvent,
    ScrollHandle, ScrollStrategy, StatefulInteractiveElement, Styled, Subscription, Task,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use gpui_base::input::{Input, InputBase, InputEvent, InputState};
use serde_json::Value;

use crate::i18n::t;
use crate::model::Cell;
use crate::results::{self, ResultId, ResultSet};

const ROW_H: f32 = 28.0;
const BAR_H: f32 = 28.0;
const MIN_W: f32 = 48.0;
const MAX_AUTO_W: f32 = 360.0;
/// Rows looked at to size the columns.
const SAMPLE: usize = 200;

/// The colors a script hands in (any it leaves out fall back to Yuzora's dark theme).
#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    pub foreground: Hsla,
    pub muted: Hsla,
    pub dim: Hsla,
    pub divider: Hsla,
    pub header: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
    pub edited_row: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub button: Hsla,
    pub button_text: Hsla,
    pub button_border: Hsla,
    pub background: Hsla,
}

pub(crate) fn hex(s: &str) -> Option<Hsla> {
    let s = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(s, 16).ok()?;
    let rgba = match s.len() {
        6 => gpui::rgb(v),
        8 => gpui::rgba(v),
        _ => return None,
    };
    Some(rgba.into())
}

impl Colors {
    pub fn from_props(colors: Option<&Value>) -> Self {
        let pick = |key: &str, default: &str| {
            colors.and_then(|c| c.get(key)).and_then(Value::as_str).and_then(hex).unwrap_or_else(|| hex(default).unwrap())
        };
        Self {
            foreground: pick("foreground", "#e4e4e7"),
            muted: pick("muted", "#a1a1aa"),
            dim: pick("dim", "#8b8b94"),
            divider: pick("divider", "#202125"),
            header: pick("header", "#1c1d21"),
            hover: pick("hover", "#1f2024"),
            selected: pick("selected", "#26272c"),
            accent: pick("accent", "#60a5fa"),
            edited_row: pick("editedRow", "#1a2333"),
            warning: pick("warning", "#f5b453"),
            error: pick("error", "#f08a8a"),
            button: pick("button", "#60a5fa"),
            button_text: pick("buttonText", "#0b1220"),
            button_border: pick("buttonBorder", "#34353a"),
            background: pick("background", "#131417"),
        }
    }
}

/// A cell position in the view: (shown row, column).
type Pos = (usize, usize);

struct Editing {
    /// Index into the result's `rows` (not the view).
    row: usize,
    column: usize,
    /// What the editor opened with: left as it was, nothing is edited.
    initial: String,
    /// The cell as it was (NULL opens the editor empty).
    original: Cell,
    /// Typed into: a NULL cell left blank then means an empty string.
    typed: bool,
    input: Entity<InputState>,
    _events: Subscription,
}

pub struct Grid {
    result: Option<ResultId>,
    set: Option<Arc<Mutex<ResultSet>>>,
    widths: Vec<f32>,
    /// Rows looked at when the widths were last sized (they grow while rows stream in).
    sized_at: usize,
    list: UniformListScrollHandle,
    hscroll: ScrollHandle,
    anchor: Option<Pos>,
    head: Option<Pos>,
    editing: Option<Editing>,
    /// (column, pointer x at the press, width at the press).
    resizing: Option<(usize, f32, f32)>,
    focus: FocusHandle,
    colors: Colors,
    /// Why the last cell asked for couldn't be edited here.
    notice: Option<String>,
    /// The footer's message is open in full (it is one truncated line otherwise).
    message_open: bool,
    /// The write failure already shown: a new one opens the message by itself.
    failure_seen: Option<String>,
    /// Frames drawn before the list had a size (the footer's range waits for one).
    unsized_frames: u8,
    _watch: Option<Task<()>>,
}

impl Focusable for Grid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

thread_local! {
    /// Grids by id, kept while their pane lives (not only while drawn: a pane
    /// in another tab keeps its scroll and widths). [`release`] drops them.
    static GRIDS: RefCell<HashMap<String, Entity<Grid>>> = RefCell::new(HashMap::new());
}

/// Drops the grids whose id starts with `prefix` (`"<owner>:"`).
pub fn release(prefix: &str) {
    GRIDS.with(|all| all.borrow_mut().retain(|id, _| !id.starts_with(prefix)));
}

/// The element for `DataGrid(id, {result, colors})`.
pub fn element(id: &str, props: &Value, _window: &mut Window, cx: &mut App) -> AnyElement {
    let result = props.get("result").and_then(Value::as_u64);
    let colors = Colors::from_props(props.get("colors"));
    let grid = GRIDS.with(|all| all.borrow().get(id).cloned()).unwrap_or_else(|| {
        let grid = cx.new(Grid::new);
        GRIDS.with(|all| all.borrow_mut().insert(id.to_owned(), grid.clone()));
        grid
    });
    grid.update(cx, |g, cx| {
        if g.colors != colors {
            g.colors = colors;
            cx.notify();
        }
        g.show(result, cx);
    });
    div().size_full().min_h_0().child(grid).into_any_element()
}

impl Grid {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            result: None,
            set: None,
            widths: Vec::new(),
            sized_at: 0,
            list: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            anchor: None,
            head: None,
            editing: None,
            resizing: None,
            focus: cx.focus_handle(),
            colors: Colors::from_props(None),
            notice: None,
            message_open: false,
            failure_seen: None,
            unsized_frames: 0,
            _watch: None,
        }
    }

    /// Points the grid at another result (or none).
    fn show(&mut self, result: Option<ResultId>, cx: &mut Context<Self>) {
        if self.result == result {
            return;
        }
        self.result = result;
        self.set = result.and_then(results::get);
        self.widths.clear();
        self.sized_at = 0;
        self.anchor = None;
        self.head = None;
        self.editing = None;
        self.notice = None;
        self.message_open = false;
        self.failure_seen = None;
        self.unsized_frames = 0;
        self.resizing = None;
        self.list = UniformListScrollHandle::new();
        self.hscroll = ScrollHandle::new();
        self._watch = result.and_then(results::watch).map(|rx| {
            cx.spawn(async move |this, cx| {
                while rx.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }

    /// Sizes columns from their names and the first rows; grows them while
    /// rows stream in, never shrinks one the person resized.
    fn size_columns(&mut self, r: &ResultSet) {
        let sample = r.rows.len().min(SAMPLE);
        if self.widths.len() == r.columns.len() && self.sized_at >= sample {
            return;
        }
        let fresh = self.widths.len() != r.columns.len();
        let widths: Vec<f32> = r
            .columns
            .iter()
            .enumerate()
            .map(|(c, col)| {
                let header = text_width(&col.name) + 22.0;
                let cells = r.rows[..sample]
                    .iter()
                    .filter_map(|row| row.get(c))
                    .map(|cell| text_width(&cell.display()))
                    .fold(0.0_f32, f32::max)
                    + 20.0;
                header.max(cells).clamp(MIN_W, MAX_AUTO_W)
            })
            .collect();
        if fresh {
            self.widths = widths;
        } else {
            for (w, auto) in self.widths.iter_mut().zip(widths) {
                *w = w.max(auto);
            }
        }
        self.sized_at = sample;
    }

    fn selection(&self) -> Option<(Range<usize>, Range<usize>)> {
        let (a, h) = (self.anchor?, self.head?);
        Some((a.0.min(h.0)..a.0.max(h.0) + 1, a.1.min(h.1)..a.1.max(h.1) + 1))
    }

    fn is_selected(&self, pos: Pos) -> bool {
        self.selection().is_some_and(|(rows, cols)| rows.contains(&pos.0) && cols.contains(&pos.1))
    }

    fn select(&mut self, pos: Pos, extend: bool, cx: &mut Context<Self>) {
        if !extend || self.anchor.is_none() {
            self.anchor = Some(pos);
        }
        self.head = Some(pos);
        self.notice = None;
        self.list.scroll_to_item(pos.0, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let (Some((rows, cols)), Some(set)) = (self.selection(), self.set.as_ref()) else { return };
        let r = results::lock(set);
        let mut out = String::new();
        for view_row in rows {
            if view_row >= r.len() {
                break;
            }
            let row = r.row_index(view_row);
            let line: Vec<String> = cols
                .clone()
                .map(|c| r.cell(row, c).map(|v| tsv(&v.copy_text())).unwrap_or_default())
                .collect();
            out.push_str(&line.join("\t"));
            out.push('\n');
        }
        out.pop();
        cx.write_to_clipboard(ClipboardItem::new_string(out));
    }

    fn sort(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(set) = self.set.clone() else { return };
        self.commit_editing(window, cx);
        let next = match results::lock(&set).sort {
            Some((c, false)) if c == column => Some((column, true)),
            Some((c, true)) if c == column => None,
            _ => Some((column, false)),
        };
        results::lock(&set).sort_by(next);
        self.anchor = None;
        self.head = None;
        cx.notify();
    }

    fn start_editing(&mut self, pos: Pos, window: &mut Window, cx: &mut Context<Self>) {
        let Some(set) = self.set.clone() else { return };
        let outcome = {
            let r = results::lock(&set);
            if pos.0 >= r.len() || pos.1 >= r.columns.len() {
                return;
            }
            if let Some(why) = edit_blocked(&r) {
                self.notice = Some(why);
                cx.notify();
                return;
            }
            let row = r.row_index(pos.0);
            if !r.column_editable(pos.1) {
                self.notice = Some(t("grid.generated", &[]));
                cx.notify();
                return;
            }
            // The editor is one line of text: a value it can't hold whole
            // (line breaks, bytes) would come back changed.
            let cell = r.cell(row, pos.1);
            if cell.is_some_and(|c| matches!(c, Cell::Bytes(_)) || matches!(c, Cell::Text(s) if s.contains(['\n', '\r']))) {
                Err(t("grid.not_inline", &[]))
            } else {
                let original = cell.cloned().unwrap_or(Cell::Null);
                Ok((row, cell.filter(|c| !c.is_null()).map(|c| c.display().into_owned()).unwrap_or_default(), original))
            }
        };
        let (row, value, original) = match outcome {
            Ok(found) => found,
            Err(why) => {
                self.notice = Some(why);
                cx.notify();
                return;
            }
        };
        self.commit_editing(window, cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value.clone()));
        input.update(cx, |state, cx| {
            state.select_all(window, cx);
            state.focus(window, cx);
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                // Enter while an input method composes chooses its candidate; it is not the cell being kept.
                if this.composing(window, cx) {
                    return;
                }
                if let Some(e) = this.editing.as_mut() {
                    // Enter on a blank NULL cell asks for an empty string.
                    e.typed = true;
                }
                this.commit_editing(window, cx);
                this.focus.focus(window, cx);
            }
            InputEvent::Change => {
                if let Some(e) = this.editing.as_mut() {
                    e.typed = true;
                }
            }
            InputEvent::Blur => this.commit_editing(window, cx),
            _ => {}
        });
        self.editing = Some(Editing { row, column: pos.1, initial: value, original, typed: false, input, _events: events });
        cx.notify();
    }

    /// Whether an input method is composing in the editor (marked text it has
    /// not been told to keep yet: bopomofo or pinyin waiting for a candidate).
    /// That text is not typed yet, and what Enter and the save key mean while it
    /// is there is the input method's to say.
    fn composing(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.editing.as_ref().is_some_and(|e| marked_text(&e.input, window, cx).is_some())
    }

    /// Keeps what the inline editor holds as a pending edit. This is the one
    /// place the editor's text becomes a cell, whatever asked for it (Write
    /// back, another cell, a header, the editor losing focus), and the text an
    /// input method is still composing is not part of it.
    fn commit_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(editing), Some(set)) = (self.editing.take(), self.set.clone()) else { return };
        let marked = marked_text(&editing.input, window, cx);
        let value = without_marked(&editing.input.read(cx).value(), marked);
        let (kept, id) = {
            let mut r = results::lock(&set);
            let kept = r.type_into(editing.row, editing.column, &editing.original, &editing.initial, value, editing.typed);
            (kept, r.id)
        };
        // Opened and left: a NULL stays NULL, 5 stays the number 5.
        if kept {
            results::changed(id);
        }
        cx.notify();
    }

    fn cancel_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn set_null(&mut self, cx: &mut Context<Self>) {
        let (Some(set), Some((rows, cols))) = (self.set.clone(), self.selection()) else { return };
        let id = {
            let mut r = results::lock(&set);
            if let Some(why) = edit_blocked(&r) {
                self.notice = Some(why);
                cx.notify();
                return;
            }
            for view_row in rows {
                if view_row >= r.len() {
                    break;
                }
                let row = r.row_index(view_row);
                for c in cols.clone() {
                    r.set_edit(row, c, Cell::Null);
                }
            }
            r.id
        };
        results::changed(id);
        cx.notify();
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        let Some(set) = self.set.clone() else { return };
        let id = {
            let mut r = results::lock(&set);
            r.discard_edits();
            r.id
        };
        results::changed(id);
        cx.notify();
    }

    fn write_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_editing(window, cx);
        let Some(set) = self.set.clone() else { return };
        if results::lock(&set).edits.is_empty() {
            return;
        }
        crate::runtime::spawn(async move {
            if let Err(error) = crate::session::write_back_set(&set).await {
                // `write_back_set` records it itself; this only makes sure the footer has it.
                let id = {
                    let mut r = results::lock(&set);
                    r.show_failure(&error);
                    r.id
                };
                results::changed(id);
            }
        });
    }

    fn stop_write(&mut self, cx: &mut Context<Self>) {
        let Some(set) = self.set.clone() else { return };
        results::lock(&set).stop_write();
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if self.editing.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.cancel_editing(window, cx);
                    cx.stop_propagation();
                }
                // Saving with the editor still open keeps the cell first: the
                // same as Enter, then Write back. Not while an input method is
                // composing: the unchosen text would be written as the cell.
                "s" if m.secondary() => {
                    if !self.composing(window, cx) {
                        self.write_back(window, cx);
                        self.focus.focus(window, cx);
                    }
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }
        let Some(set) = self.set.clone() else { return };
        let (rows, cols) = {
            let r = results::lock(&set);
            (r.len(), r.columns.len())
        };
        if rows == 0 || cols == 0 {
            return;
        }
        let head = self.head.unwrap_or((0, 0));
        let moved = match event.keystroke.key.as_str() {
            "up" => Some((head.0.saturating_sub(1), head.1)),
            "down" => Some(((head.0 + 1).min(rows - 1), head.1)),
            "left" => Some((head.0, head.1.saturating_sub(1))),
            "right" | "tab" => Some((head.0, (head.1 + 1).min(cols - 1))),
            "pageup" => Some((head.0.saturating_sub(20), head.1)),
            "pagedown" => Some(((head.0 + 20).min(rows - 1), head.1)),
            "home" if m.secondary() => Some((0, head.1)),
            "end" if m.secondary() => Some((rows - 1, head.1)),
            _ => None,
        };
        if let Some(pos) = moved {
            self.select(pos, m.shift, cx);
            // The list scrolls at the next layout: the footer's range follows then.
            cx.on_next_frame(window, |_, _, cx| cx.notify());
            cx.stop_propagation();
            return;
        }
        match event.keystroke.key.as_str() {
            "c" if m.secondary() => self.copy(cx),
            "s" if m.secondary() => self.write_back(window, cx),
            "a" if m.secondary() => {
                self.anchor = Some((0, 0));
                self.head = Some((rows - 1, cols - 1));
                cx.notify();
            }
            "enter" if !m.secondary() => self.start_editing(head, window, cx),
            "backspace" | "delete" if m.secondary() => self.set_null(cx),
            "escape" => {
                self.anchor = None;
                self.head = None;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn header(&self, r: &ResultSet, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.colors;
        let mut row = div().flex().flex_row().h(px(BAR_H)).flex_none().bg(c.header).text_color(c.muted).text_size(px(11.5));
        for (i, col) in r.columns.iter().enumerate() {
            let arrow = match r.sort {
                Some((s, false)) if s == i => " ↑",
                Some((s, true)) if s == i => " ↓",
                _ => "",
            };
            let width = self.widths.get(i).copied().unwrap_or(120.0);
            let label = div()
                .id(("h", i))
                .flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .px(px(10.0))
                .gap(px(5.0))
                .cursor_pointer()
                .hover(|s| s.text_color(c.foreground))
                .when(col.numeric, |d| d.justify_end())
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.sort(i, window, cx)))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).truncate().child(format!("{}{arrow}", col.name)));
            let handle = div()
                .id(("rz", i))
                .w(px(6.0))
                .h_full()
                .flex_none()
                .cursor_col_resize()
                .hover(|s| s.bg(c.divider))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        let width = this.widths.get(i).copied().unwrap_or(120.0);
                        this.resizing = Some((i, f32::from(e.position.x), width));
                        cx.stop_propagation();
                    }),
                );
            row = row.child(div().flex().flex_row().w(px(width)).flex_none().h_full().child(label).child(handle));
        }
        row
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(set) = self.set.clone() else { return Vec::new() };
        let r = results::lock(&set);
        let c = self.colors.clone();
        let editable = r.editable() && !r.is_writing();
        range
            .filter(|i| *i < r.len())
            .map(|view_row| {
                let row = r.row_index(view_row);
                let edited_row = r.edits.range((row as u32, 0)..(row as u32 + 1, 0)).next().is_some();
                let mut line = div()
                    .id(("r", view_row))
                    .flex()
                    .flex_row()
                    .h(px(ROW_H))
                    .items_center()
                    .border_b_1()
                    .border_color(c.divider)
                    .when(edited_row, |d| d.bg(c.edited_row));
                for (col_ix, col) in r.columns.iter().enumerate() {
                    let width = self.widths.get(col_ix).copied().unwrap_or(120.0);
                    let editing_here = self.editing.as_ref().filter(|e| e.row == row && e.column == col_ix);
                    let selected = self.is_selected((view_row, col_ix));
                    let cell = div()
                        .id(("c", view_row * 4096 + col_ix))
                        .debug_selector(move || format!("grid-cell-{view_row}-{col_ix}"))
                        .w(px(width))
                        .flex_none()
                        .h_full()
                        .flex()
                        .items_center()
                        .px(px(10.0))
                        .overflow_hidden()
                        .when(selected && editing_here.is_none(), |d| d.bg(c.selected))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                if this.editing.as_ref().is_some_and(|ed| ed.row == row && ed.column == col_ix) {
                                    return;
                                }
                                this.commit_editing(window, cx);
                                this.focus.focus(window, cx);
                                this.select((view_row, col_ix), e.modifiers.shift, cx);
                            }),
                        )
                        .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                            if e.click_count() >= 2 {
                                this.start_editing((view_row, col_ix), window, cx);
                            }
                        }));
                    let cell = if let Some(editing) = editing_here {
                        let focus_target = editing.input.clone();
                        cell.px(px(4.0)).child(
                            InputBase::new(("yuzora-db-grid-input", view_row * 4096 + col_ix))
                                .flex()
                                .items_center()
                                .w_full()
                                .h(px(22.0))
                                .px(px(6.0))
                                .rounded(px(3.0))
                                .border(px(1.5))
                                .border_color(c.accent)
                                .bg(c.background)
                                .text_color(c.foreground)
                                .text_size(px(12.0))
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    focus_target.update(cx, |s, cx| s.focus(window, cx));
                                })
                                .child(Input::new(&editing.input)),
                        )
                    } else {
                        let value = r.cell(row, col_ix);
                        let edited = r.is_edited(row, col_ix);
                        let text = value.map(|v| one_line(&v.display())).unwrap_or_default();
                        let null = value.is_none_or(Cell::is_null);
                        cell.when(col.numeric, |d| d.justify_end())
                            .text_color(if edited {
                                c.warning
                            } else if null {
                                c.dim
                            } else {
                                c.foreground
                            })
                            .when(null, |d| d.italic())
                            .when(editable, |d| d.cursor_text())
                            .child(div().truncate().child(text))
                    };
                    line = line.child(cell);
                }
                line.into_any_element()
            })
            .collect()
    }

    fn footer(&self, r: &ResultSet, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.colors;
        let shown = r.len();
        let (first, last) = self.visible(shown);
        let rows_text = match (r.total, r.complete, r.truncated) {
            (Some(total), _, _) => t("grid.rows", &[("count", thousands(total))]),
            (None, false, _) => t("grid.rows_loading", &[("count", thousands(shown as u64))]),
            (None, true, true) => t("grid.rows_capped", &[("count", thousands(shown as u64))]),
            (None, true, false) => t("grid.rows", &[("count", thousands(shown as u64))]),
        };
        let mut parts = vec![rows_text];
        if shown > 0 {
            parts.push(t("grid.showing", &[("first", thousands(first as u64)), ("last", thousands(last as u64))]));
        }
        if r.total.is_some_and(|t| t > shown as u64) {
            parts.push(t("grid.loaded", &[("count", thousands(shown as u64))]));
        }
        parts.push(format!("{} ms", r.elapsed_ms));
        let edits = r.edits.len();
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .debug_selector(move || format!("grid-{id}"))
                .h(px(20.0))
                .px(px(8.0))
                .flex()
                .flex_none()
                .max_w_full()
                .items_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .when(primary, |d| d.bg(c.button).text_color(c.button_text).font_weight(gpui::FontWeight::SEMIBOLD))
                .when(!primary, |d| d.border_1().border_color(c.button_border).text_color(c.foreground))
                .hover(|s| s.opacity(0.88))
                .child(div().truncate().child(label))
        };
        // Each piece of status is a line of its own once the pane is too narrow
        // for it beside the others, and is cut at the pane's edge when even
        // that is too wide.
        let text = |color: Hsla, text: String| div().max_w_full().truncate().text_color(color).child(text);
        let status = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .flex_auto()
            .min_w_0()
            .items_center()
            .gap_x(px(12.0))
            .child(text(c.muted, parts.join(" · ")))
            .when(edits > 0, |d| d.child(text(c.warning, t("grid.modified", &[("count", edits.to_string())]))))
            .when_some(long_message(r), |d, (message, error)| {
                let (open, color) = (self.message_open, if error { c.error } else { c.dim });
                d.child(
                    text(color, format!("{} {}", if open { "▾" } else { "▸" }, one_line(&message)))
                        .id("message")
                        .debug_selector(|| "grid-message".into())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.message_open = !this.message_open;
                            cx.notify();
                        })),
                )
            })
            .when_some(self.notice.clone(), |d, why| d.child(text(c.dim, why)));
        // The buttons wrap rather than leave the pane: a footer that can't be
        // pressed is no way to write back or to stop.
        let writing = r.is_writing();
        let actions = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .flex_none()
            .max_w_full()
            .ml_auto()
            .items_center()
            .justify_end()
            .gap(px(6.0))
            .when(writing, |d| {
                d.child(div().flex_none().text_color(c.dim).child(t("grid.writing", &[])))
                    .child(
                        button("stop", t("grid.stop", &[]), false)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.stop_write(cx))),
                    )
            })
            .when(edits > 0 && !writing, |d| {
                d.child(
                    button("discard", t("grid.discard", &[]), false)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.discard(cx))),
                )
                .child(
                    button("write", t("grid.write", &[]), true)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.write_back(window, cx))),
                )
            });
        div()
            .id("footer")
            .debug_selector(|| "grid-footer".into())
            .flex()
            .flex_row()
            .flex_wrap()
            .flex_none()
            .min_h(px(BAR_H))
            .items_center()
            .gap_x(px(12.0))
            .gap_y(px(2.0))
            .px(px(12.0))
            .py(px(4.0))
            .text_size(px(11.5))
            .text_color(c.muted)
            .border_t_1()
            .border_color(c.divider)
            .child(status)
            .when(writing || edits > 0, |d| d.child(actions))
    }

    /// The first and last rows on screen (1-based).
    fn visible(&self, rows: usize) -> (usize, usize) {
        if rows == 0 {
            return (0, 0);
        }
        let state = self.list.0.borrow();
        let offset = -f32::from(state.base_handle.offset().y);
        let height = f32::from(state.base_handle.bounds().size.height);
        let first = (offset / ROW_H).floor().max(0.0) as usize;
        // Before the list is laid out its height is 0: one row, until render asks again.
        let count = ((height / ROW_H).ceil() as usize).max(1);
        ((first + 1).min(rows), (first + count).min(rows))
    }
}

impl Render for Grid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors.clone();
        let root = div()
            .id("yuzora-db-grid")
            .track_focus(&self.focus)
            .key_context("YuzoraDataGrid")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .text_color(c.foreground)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let Some((column, start, width)) = this.resizing {
                    if e.pressed_button != Some(MouseButton::Left) {
                        this.resizing = None;
                        return;
                    }
                    if let Some(w) = this.widths.get_mut(column) {
                        *w = (width + f32::from(e.position.x) - start).max(MIN_W);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.resizing = None));
        let Some(set) = self.set.clone() else {
            return root;
        };
        let (count, width, header, footer, empty, message) = {
            let r = results::lock(&set);
            self.size_columns(&r);
            match r.write_failure() {
                // A new failure opens by itself: it is what the person is waiting for.
                Some(failure) if self.failure_seen.as_deref() != Some(failure) => {
                    self.failure_seen = Some(failure.to_owned());
                    self.message_open = true;
                }
                Some(_) => {}
                None => self.failure_seen = None,
            }
            let width: f32 = self.widths.iter().sum();
            let empty = r.columns.is_empty();
            (
                r.len(),
                width,
                self.header(&r, cx).into_any_element(),
                self.footer(&r, cx).into_any_element(),
                empty,
                long_message(&r).filter(|_| self.message_open),
            )
        };
        let body = if empty {
            div().flex_1().into_any_element()
        } else {
            div()
                .id("hscroll")
                .flex_1()
                .min_h_0()
                .overflow_x_scroll()
                .track_scroll(&self.hscroll)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .h_full()
                        .w(px(width.max(1.0)))
                        .child(header)
                        .child(
                            uniform_list("rows", count, cx.processor(|this, range, _, cx| this.rows(range, cx)))
                                .track_scroll(&self.list)
                                .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, window, cx| {
                                    cx.on_next_frame(window, |_, _, cx| cx.notify());
                                }))
                                .flex_1()
                                .min_h_0(),
                        ),
                )
                .into_any_element()
        };
        let laid_out = f32::from(self.list.0.borrow().base_handle.bounds().size.height) > 0.0;
        if count > 0 && !laid_out && self.unsized_frames < 3 {
            self.unsized_frames += 1;
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let open_message = message.map(|(text, error)| {
            div()
                .id("message-open")
                .debug_selector(|| "grid-message-open".into())
                // Gives way before the footer does: the buttons below it are the way out.
                .flex_shrink_1()
                .min_h_0()
                .max_h(px(140.0))
                .overflow_y_scroll()
                .px(px(12.0))
                .py(px(6.0))
                .text_size(px(11.5))
                .text_color(if error { c.error } else { c.dim })
                .bg(c.header)
                .border_t_1()
                .border_color(c.divider)
                .child(text)
        });
        root.child(body).when_some(open_message, |d, panel| d.child(panel)).child(footer)
    }
}

/// Roughly how wide `s` draws at 12.5px: CJK counts double.
fn text_width(s: &str) -> f32 {
    s.chars().take(80).map(|ch| if ch.len_utf8() >= 3 { 12.5 } else { 7.0 }).sum()
}

fn one_line(s: &str) -> String {
    let mut out: String = s.chars().take(500).collect();
    if out.contains(['\n', '\r', '\t']) {
        out = out.replace("\r\n", " ⏎ ").replace(['\n', '\r'], " ⏎ ").replace('\t', " ");
    }
    out
}

/// A value safe inside tab-separated text: quoted when it holds a tab, newline or quote.
fn tsv(s: &str) -> String {
    if s.contains(['\t', '\n', '\r', '"']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
}

/// `2104` → `2,104`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The one message the footer carries, in full (the footer shows its first
/// line, a click opens the rest above it): the write failure, else why the
/// result is read-only, else what stopped the read. `true`: an error.
fn long_message(r: &ResultSet) -> Option<(String, bool)> {
    if let Some(failure) = r.write_failure() {
        return Some((failure.to_owned(), true));
    }
    if let Some(why) = r.read_only_reason().filter(|_| r.edits.is_empty()) {
        return Some((why, false));
    }
    r.error.clone().filter(|_| r.columns.is_empty()).map(|e| (e, true))
}

/// Why an edit can't start now (`None`: it can).
fn edit_blocked(r: &ResultSet) -> Option<String> {
    if r.is_writing() { Some(t("edit.writing", &[])) } else { r.editing.blocked() }
}

/// The input method's composition in `input`, as the UTF-16 range of the text
/// it covers.
fn marked_text(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
    input.update(cx, |input, cx| input.marked_text_range(window, cx))
}

/// `text` without its composing part (`marked`, in UTF-16 units).
fn without_marked(text: &str, marked: Option<Range<usize>>) -> String {
    let Some(marked) = marked else { return text.to_owned() };
    let mut at = 0;
    text.chars()
        .filter(|c| {
            let composing = marked.contains(&at);
            at += c.len_utf16();
            !composing
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_composition_is_no_part_of_the_text() {
        assert_eq!(without_marked("abc", None), "abc");
        // UTF-16 units: the 𠀀 before the composition is two of them, each ㄓ one.
        assert_eq!(without_marked("𠀀中ㄓㄨㄥ", Some(3..6)), "𠀀中");
        assert_eq!(without_marked("ㄓㄨㄥ", Some(0..3)), "");
        assert_eq!(without_marked("a𠀀b", Some(1..3)), "ab");
        assert_eq!(without_marked("ab", Some(1..1)), "ab");
    }

    fn result(editing: crate::results::Editing) -> std::sync::Arc<std::sync::Mutex<ResultSet>> {
        let set = results::create("grid-tests", "select");
        {
            let mut r = results::lock(&set);
            r.columns = vec![crate::model::Column::new("id", "int4")];
            r.rows = vec![vec![Cell::text("1")]];
            r.complete = true;
            r.editing = editing;
        }
        set
    }

    fn target() -> crate::results::EditTarget {
        crate::results::EditTarget {
            connection: "c".into(),
            config: serde_json::from_value(serde_json::json!({"name": "c", "engine": "sqlite", "path": "t.sqlite"})).unwrap(),
            database: None,
            owner: "grid-tests".into(),
            table: crate::model::ObjectRef { schema: String::new(), name: "t".into() },
            key: vec![0],
            read_only: Vec::new(),
        }
    }

    #[test]
    fn a_cell_that_cannot_be_edited_has_a_reason_to_show() {
        use crate::results::{Editing, ReadOnly, WriteState, WriteStop};
        let set = result(Editing::Loading);
        assert_eq!(edit_blocked(&results::lock(&set)), Some(t("edit.loading", &[])));
        for why in [ReadOnly::Query, ReadOnly::Connection, ReadOnly::NoKey, ReadOnly::NoInfo("denied".into()), ReadOnly::ReadFailed] {
            let set = result(Editing::ReadOnly(why.clone()));
            assert_eq!(edit_blocked(&results::lock(&set)), Some(why.reason()));
            assert!(!why.reason().is_empty());
        }
        let set = result(Editing::Editable(Box::new(target())));
        assert_eq!(edit_blocked(&results::lock(&set)), None);
        results::lock(&set).write = WriteState::Writing(WriteStop::new());
        assert_eq!(edit_blocked(&results::lock(&set)), Some(t("edit.writing", &[])));
    }

    #[test]
    fn the_footer_carries_a_write_failure_in_full_before_anything_else() {
        use crate::results::{Editing, ReadOnly, WriteState};
        let long = "new row violates check constraint \"c\" (23514)\nDETAIL: Failing row contains (1, x).";
        let set = result(Editing::Editable(Box::new(target())));
        assert_eq!(long_message(&results::lock(&set)), None);
        results::lock(&set).write = WriteState::failed(long);
        assert_eq!(long_message(&results::lock(&set)), Some((long.to_owned(), true)), "the whole text, DETAIL line included");
        assert_eq!(WriteState::failed("  "), WriteState::Failed(t("edit.failed", &[])), "a failure is never blank");

        let set = result(Editing::ReadOnly(ReadOnly::NoKey));
        assert_eq!(long_message(&results::lock(&set)), Some((ReadOnly::NoKey.reason(), false)));
        results::lock(&set).edits.insert((0, 0), Cell::text("x"));
        assert_eq!(long_message(&results::lock(&set)), None, "pending edits take the footer's room");
        let set = result(Editing::ReadOnly(ReadOnly::ReadFailed));
        {
            let mut r = results::lock(&set);
            r.columns.clear();
            r.error = Some("connection reset".into());
        }
        assert_eq!(long_message(&results::lock(&set)).map(|m| m.1), Some(false));
    }

    #[test]
    fn numbers_get_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2104), "2,104");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn copied_values_survive_tabs_and_newlines() {
        assert_eq!(tsv("plain"), "plain");
        assert_eq!(tsv("a\tb"), "\"a\tb\"");
        assert_eq!(tsv("say \"hi\"\n"), "\"say \"\"hi\"\"\n\"");
        assert_eq!(one_line("a\nb"), "a ⏎ b");
    }

    mod composing {
        //! An input method's composition is not typed text yet.
        use super::*;
        use gpui::{TestAppContext, VisualTestContext};

        struct Host {
            id: String,
            result: u64,
        }

        impl Render for Host {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().w(px(600.0)).h(px(300.0)).child(element(
                    &self.id,
                    &serde_json::json!({ "result": self.result }),
                    window,
                    cx,
                ))
            }
        }

        fn save_keys() -> &'static str {
            if cfg!(target_os = "macos") { "cmd-s" } else { "ctrl-s" }
        }

        fn settle(cx: &mut VisualTestContext) {
            for _ in 0..3 {
                cx.update(|window, cx| window.draw(cx).clear(cx));
                cx.run_until_parked();
            }
        }

        /// The first cell of the name column open for editing, with `typed` in the input method's composition.
        fn composing<'a>(
            cx: &'a mut TestAppContext,
            name: &str,
            typed: &str,
        ) -> (&'a mut VisualTestContext, Entity<InputState>, Arc<Mutex<ResultSet>>) {
            composing_in(cx, name, Cell::text("a"), typed)
        }

        /// The same, for a name column that holds `original`.
        fn composing_in<'a>(
            cx: &'a mut TestAppContext,
            name: &str,
            original: Cell,
            typed: &str,
        ) -> (&'a mut VisualTestContext, Entity<InputState>, Arc<Mutex<ResultSet>>) {
            let set = results::create(name, "select * from people");
            {
                let mut r = results::lock(&set);
                r.columns = vec![
                    crate::model::Column::new("id", "int4"),
                    crate::model::Column::new("name", "text"),
                    crate::model::Column::new("note", "text"),
                ];
                r.rows = vec![vec![Cell::text("1"), original, Cell::text("n")]];
                r.complete = true;
                let mut target = target();
                target.owner = name.to_owned();
                r.editing = crate::results::Editing::Editable(Box::new(target));
            }
            let (id, owner) = (results::lock(&set).id, name.to_owned());
            cx.update(gpui_base::init);
            let (_, cx) = cx.add_window_view(move |_, _| Host { id: owner, result: id });
            settle(cx);
            let grid = GRIDS.with(|g| g.borrow().get(name).cloned()).expect("the grid");
            grid.update_in(cx, |g, window, cx| g.start_editing((0, 1), window, cx));
            settle(cx);
            let input =
                grid.read_with(cx, |g, _| g.editing.as_ref().map(|e| e.input.clone())).expect("the editor is open");
            input.update_in(cx, |s, window, cx| s.replace_and_mark_text_in_range(None, typed, None, window, cx));
            settle(cx);
            assert!(
                input.update_in(cx, |s, window, cx| s.marked_text_range(window, cx)).is_some(),
                "the composition is marked"
            );
            (cx, input, set)
        }

        fn is_editing(cx: &mut VisualTestContext, name: &str) -> bool {
            let grid = GRIDS.with(|g| g.borrow().get(name).cloned()).expect("the grid");
            grid.read_with(cx, |g, _| g.editing.is_some())
        }

        #[gpui::test]
        fn saving_leaves_a_composition_to_the_input_method(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (cx, _, set) = composing(cx, "composing-save", "ㄓㄨㄥ");
            cx.simulate_keystrokes(save_keys());
            settle(cx);
            let r = results::lock(&set);
            assert!(r.edits.is_empty(), "the unchosen \"ㄓㄨㄥ\" was taken for the cell: {:?}", r.edits);
            assert_eq!(r.write, crate::results::WriteState::Idle, "and written");
            drop(r);
            assert!(is_editing(cx, "composing-save"), "the editor stays open for the composition");
        }

        /// A pending edit in a cell other than the one being edited, so the footer offers Write back.
        fn with_another_edit(cx: &mut VisualTestContext, set: &Arc<Mutex<ResultSet>>) {
            let id = {
                let mut r = results::lock(set);
                r.set_edit(0, 2, Cell::text("other cell"));
                r.id
            };
            results::changed(id);
            settle(cx);
        }

        fn click(cx: &mut VisualTestContext, selector: &'static str) {
            let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
            settle(cx);
        }

        fn edit_of(set: &Arc<Mutex<ResultSet>>, column: u32) -> Option<Cell> {
            results::lock(set).edits.get(&(0, column)).cloned()
        }

        #[gpui::test]
        fn the_write_back_button_writes_what_was_chosen_and_not_the_composition(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (cx, input, set) = composing(cx, "composing-button", "ㄓㄨㄥ");
            input.update_in(cx, |s, window, cx| s.replace_text_in_range(None, "𠀀中", window, cx));
            input.update_in(cx, |s, window, cx| s.replace_and_mark_text_in_range(None, "ㄅ", None, window, cx));
            with_another_edit(cx, &set);
            click(cx, "grid-write");
            assert_eq!(
                edit_of(&set, 1),
                Some(Cell::text("𠀀中")),
                "the cell is what was chosen, without the composition"
            );
            assert_eq!(edit_of(&set, 2), Some(Cell::text("other cell")));
            assert!(!is_editing(cx, "composing-button"));
        }

        #[gpui::test]
        fn leaving_a_cell_keeps_what_was_chosen_and_not_the_composition(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (cx, input, set) = composing(cx, "composing-leave", "ㄓㄨㄥ");
            input.update_in(cx, |s, window, cx| s.replace_text_in_range(None, "中", window, cx));
            input.update_in(cx, |s, window, cx| s.replace_and_mark_text_in_range(None, "ㄅ", None, window, cx));
            let bounds = cx.debug_bounds("grid-cell-0-0").expect("the id cell");
            cx.simulate_mouse_down(bounds.center(), gpui::MouseButton::Left, gpui::Modifiers::none());
            settle(cx);
            assert_eq!(edit_of(&set, 1), Some(Cell::text("中")));
            assert!(!is_editing(cx, "composing-leave"));
        }

        #[gpui::test]
        fn the_editor_losing_focus_keeps_what_was_chosen_and_not_the_composition(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (cx, input, set) = composing(cx, "composing-blur", "ㄓㄨㄥ");
            input.update_in(cx, |s, window, cx| s.replace_text_in_range(None, "中", window, cx));
            input.update_in(cx, |s, window, cx| s.replace_and_mark_text_in_range(None, "ㄅ", None, window, cx));
            input.update_in(cx, |_, _, cx| cx.emit(InputEvent::Blur));
            settle(cx);
            assert_eq!(edit_of(&set, 1), Some(Cell::text("中")));
        }

        #[gpui::test]
        fn a_null_cell_that_was_only_composed_in_stays_null(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (cx, _, set) = composing_in(cx, "composing-null", Cell::Null, "ㄓㄨㄥ");
            let bounds = cx.debug_bounds("grid-cell-0-0").expect("the id cell");
            cx.simulate_mouse_down(bounds.center(), gpui::MouseButton::Left, gpui::Modifiers::none());
            settle(cx);
            assert_eq!(edit_of(&set, 1), None, "an unchosen composition made the NULL something");
        }

        /// Enter reaching the input (an input method that did not take it for itself).
        fn press_enter(cx: &mut VisualTestContext, input: &Entity<InputState>) {
            input.update_in(cx, |_, _, cx| cx.emit(InputEvent::PressEnter { secondary: false, shift: false }));
            settle(cx);
        }

        #[gpui::test]
        fn enter_keeps_the_cell_only_once_the_composition_is_chosen(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (cx, input, set) = composing(cx, "composing-enter", "ㄓㄨㄥ");
            press_enter(cx, &input);
            assert!(results::lock(&set).edits.is_empty(), "Enter while composing kept the unchosen text");
            assert!(is_editing(cx, "composing-enter"));

            input.update_in(cx, |s, window, cx| s.replace_text_in_range(None, "中", window, cx));
            settle(cx);
            press_enter(cx, &input);
            assert_eq!(results::lock(&set).edits.get(&(0, 1)), Some(&Cell::text("中")));
            assert!(!is_editing(cx, "composing-enter"));
        }
    }

    #[test]
    fn colors_fall_back_per_key() {
        let c = Colors::from_props(Some(&serde_json::json!({ "accent": "#ff0000", "muted": "nope" })));
        assert_eq!(c.accent, hex("#ff0000").unwrap());
        assert_eq!(c.muted, hex("#a1a1aa").unwrap());
        assert!(hex("#11223344").is_some());
        assert!(hex("123").is_none());
    }
}
