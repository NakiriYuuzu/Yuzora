//! The reusable code editor: a gpui-base `EditorState` with tree-sitter
//! highlighting, line numbers and folding, and the frame it is drawn in.
//!
//! Other crates build theirs with [`new_code_editor`] and draw it with
//! [`code_editor`] (L3's SQL editor, say); [`crate::docs`] uses the same two
//! for files.

use std::sync::Arc;

use gpui::{
    App, AppContext as _, ElementId, Entity, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, Styled as _, Window, px,
};
use gpui_base::input::{Editor, EditorState, InputBase, InputEditorStyle};
use yuzora_core::HostValue;

use crate::highlight::{self, Lang, SyntaxPalette};

/// How an editor looks: the person's code font and the theme's colors.
/// Anything unset follows gpui-base's palette.
#[derive(Clone)]
pub struct Look {
    pub font_family: Option<SharedString>,
    pub font_size: f32,
    pub foreground: Option<Hsla>,
    pub muted: Option<Hsla>,
    pub background: Option<Hsla>,
    pub active_line: Option<Hsla>,
    pub selection: Option<Hsla>,
    pub caret: Option<Hsla>,
    pub border: Option<Hsla>,
    /// The status bar's text.
    pub status: Option<Hsla>,
    pub syntax: Arc<SyntaxPalette>,
    /// The script's syntax colors `syntax` was built with.
    overrides: Vec<(String, Hsla)>,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: 12.5,
            foreground: None,
            muted: None,
            background: None,
            active_line: None,
            selection: None,
            caret: None,
            border: None,
            status: None,
            syntax: Arc::new(SyntaxPalette::one_dark()),
            overrides: Vec::new(),
        }
    }
}

impl PartialEq for Look {
    fn eq(&self, other: &Self) -> bool {
        self.font_family == other.font_family
            && self.font_size == other.font_size
            && self.foreground == other.foreground
            && self.muted == other.muted
            && self.background == other.background
            && self.active_line == other.active_line
            && self.selection == other.selection
            && self.caret == other.caret
            && self.border == other.border
            && self.status == other.status
            && Arc::ptr_eq(&self.syntax, &other.syntax)
    }
}

fn color(v: Option<&HostValue>) -> Option<Hsla> {
    v.and_then(HostValue::as_str).and_then(highlight::parse_hex)
}

impl Look {
    /// From a script's props: `{font_family, font_size, colors: {foreground, muted,
    /// background, active_line, selection, caret, border, status}, syntax: {name: "#hex"}}`.
    /// `previous` keeps its palette when the overrides are the same, so a
    /// re-render does not rebuild every highlight.
    pub fn from_props(props: &HostValue, previous: Option<&Look>) -> Self {
        let colors = props.get("colors");
        let get = |k: &str| color(colors.and_then(|c| c.get(k)));
        let overrides: Vec<(String, Hsla)> = props
            .get("syntax")
            .and_then(HostValue::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| Some((k.clone(), color(Some(v))?)))
                    .collect()
            })
            .unwrap_or_default();
        let syntax = match previous {
            Some(p) if p.overrides == overrides => p.syntax.clone(),
            _ => Arc::new(SyntaxPalette::one_dark().with_overrides(overrides.clone())),
        };
        Self {
            font_family: props
                .get("font_family")
                .and_then(HostValue::as_str)
                .filter(|f| !f.is_empty())
                .map(|f| SharedString::from(f.to_owned())),
            font_size: props
                .get("font_size")
                .and_then(HostValue::as_number)
                .filter(|n| *n >= 6.0 && *n <= 48.0)
                .unwrap_or(12.5) as f32,
            foreground: get("foreground"),
            muted: get("muted"),
            background: get("background"),
            active_line: get("active_line"),
            selection: get("selection"),
            caret: get("caret"),
            border: get("border"),
            status: get("status"),
            syntax,
            overrides,
        }
    }

    /// The style gpui-base paints the editor with.
    pub fn editor_style(&self) -> InputEditorStyle {
        let unset = Hsla::default();
        InputEditorStyle {
            foreground: self.foreground.unwrap_or(unset),
            muted_foreground: self.muted.unwrap_or(unset),
            background: self.background.unwrap_or(unset),
            border: self.border.unwrap_or(unset),
            selection: self.selection.unwrap_or(unset),
            caret: self.caret.unwrap_or(unset),
            highlight_styles: self.syntax.clone(),
            editor_active_line: self.active_line,
            ..InputEditorStyle::default()
        }
    }

    pub fn font(&self, cx: &App) -> SharedString {
        self.font_family
            .clone()
            .unwrap_or_else(|| gpui_base::Theme::global(cx).tokens.typography.mono.clone())
    }
}

/// What a new editor starts with.
pub struct CodeEditorOptions<'a> {
    /// `None`: plain text (no highlighting).
    pub lang: Option<Lang>,
    pub text: &'a str,
    pub readonly: bool,
    pub line_numbers: bool,
    /// Off for very large files: folding asks the parser for every block.
    pub folding: bool,
    pub soft_wrap: bool,
}

impl Default for CodeEditorOptions<'_> {
    fn default() -> Self {
        Self {
            lang: None,
            text: "",
            readonly: false,
            line_numbers: true,
            folding: true,
            soft_wrap: false,
        }
    }
}

/// A code editor's state, highlighted by [`crate::highlight`].
///
/// Its built-in search panel stays off, so Cmd/Ctrl-F reaches the owner,
/// which drives the same search engine (`set_search_query`, …) from its own UI.
pub fn new_code_editor(
    options: CodeEditorOptions<'_>,
    look: &Look,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let language = options.lang.map(Lang::id).unwrap_or("");
    // The text goes in as the default value, which needs no window: this runs
    // while a component is drawn, and the parse waits for the editor's first render.
    let text = options.text.to_owned();
    cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language(language)
            .line_number(options.line_numbers)
            .folding(options.folding)
            .soft_wrap(options.soft_wrap)
            .searchable(false)
            .default_value(text);
        state.set_readonly(options.readonly, cx);
        state.set_editor_style(look.editor_style());
        state.set_highlighter_factory(highlight::highlighter_factory(), cx);
        state
    })
}

/// The frame an editor is drawn in: fills its parent, takes the person's
/// code font, and moves the keyboard onto the editor when pressed.
pub fn code_editor(
    id: impl Into<ElementId>,
    state: &Entity<EditorState>,
    look: &Look,
    cx: &App,
) -> impl IntoElement {
    let focus_target = state.clone();
    InputBase::new(id)
        .flex()
        .items_start()
        .w_full()
        .flex_1()
        .min_h_0()
        .font_family(look.font(cx))
        .text_size(px(look.font_size))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            focus_target.update(cx, |state, cx| state.focus(window, cx));
        })
        .child(Editor::new(state))
}
