//! (L5) Tree-sitter syntax highlighting for the code editor, reusable by other
//! crates (the SQL editor) and by Markdown code blocks.
//!
//! Three entry points share one core: [`TreeSitterHighlighter`] plugs into
//! gpui-base's editor, [`highlight`] is the stateless span API, and
//! [`code_block_highlighter`] serves Markdown code blocks.

mod languages;
mod palette;
#[cfg(test)]
mod tests;

use std::{cell::RefCell, cmp::Reverse, collections::HashMap, ops::Range, rc::Rc, sync::Arc};

use gpui::{Context, HighlightStyle, SharedString, Window};
use gpui_base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, QueryCursor, Tree};

pub use languages::Lang;
pub use palette::{SyntaxPalette, parse_hex, semantic_name};

/// Sources larger than this are not parsed; the editor shows them unstyled.
pub const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;

/// A semantic span: a byte range and the name of the palette entry it uses.
type Run = (Range<usize>, Option<&'static str>);

/// Parser state for one document: the grammar, the last tree and the text it
/// was parsed from. Independent of GPUI so it can be exercised directly.
struct Syntax {
    lang: Lang,
    parser: Parser,
    tree: Option<Tree>,
    /// Empty while the source is over [`MAX_HIGHLIGHT_BYTES`].
    source: String,
    /// Byte length of the document, even when `source` is not kept.
    len: usize,
}

impl Syntax {
    fn new(lang: Lang) -> Self {
        let mut parser = Parser::new();
        // A grammar built for another runtime ABI leaves the parser without a
        // language; it then never produces a tree and the text stays unstyled.
        let _ = parser.set_language(&lang.ts_language());
        Self {
            lang,
            parser,
            tree: None,
            source: String::new(),
            len: 0,
        }
    }

    /// Records a document too large to parse and drops any tree.
    fn disable(&mut self, len: usize) {
        self.tree = None;
        self.source = String::new();
        self.len = len;
    }

    /// Parses `source`. When `edits` describe the change from the previously
    /// parsed text, the old tree is edited and reused; otherwise it is a full
    /// parse. Edits that do not add up to the new length are not trusted.
    fn reparse(&mut self, source: String, edits: &[InputEdit]) {
        if source.len() > MAX_HIGHLIGHT_BYTES {
            self.disable(source.len());
            return;
        }
        let reuse = !edits.is_empty() && self.tree.is_some() && self.edits_fit(edits, source.len());
        if reuse {
            if let Some(tree) = self.tree.as_mut() {
                for edit in edits {
                    tree.edit(&to_ts_edit(edit));
                }
            }
        } else {
            self.tree = None;
        }
        self.len = source.len();
        self.tree = self.parser.parse(&source, self.tree.as_ref());
        self.source = source;
    }

    /// Whether `edits` turn the parsed text into one of `new_len` bytes.
    fn edits_fit(&self, edits: &[InputEdit], new_len: usize) -> bool {
        let mut len = self.len;
        for edit in edits {
            if edit.start_byte > edit.old_end_byte
                || edit.start_byte > edit.new_end_byte
                || edit.old_end_byte > len
            {
                return false;
            }
            len = len - edit.old_end_byte + edit.new_end_byte;
        }
        len == new_len
    }

    /// Gap-free runs covering `range`, clamped to the document. `None` names
    /// are unstyled gaps.
    fn runs(&self, range: &Range<usize>) -> Vec<Run> {
        let start = range.start.min(self.len);
        let end = range.end.min(self.len);
        if start >= end {
            return Vec::new();
        }
        match &self.tree {
            Some(tree) => runs_for(self.lang, tree, &self.source, start..end),
            None => vec![(start..end, None)],
        }
    }

    fn fold_ranges(&self) -> Vec<FoldRange> {
        self.tree
            .as_ref()
            .map(|tree| fold_ranges_of(self.lang, tree))
            .unwrap_or_default()
    }
}

fn to_ts_point(point: gpui_base::input::Point) -> tree_sitter::Point {
    tree_sitter::Point {
        row: point.row,
        column: point.column,
    }
}

fn to_ts_edit(edit: &InputEdit) -> tree_sitter::InputEdit {
    tree_sitter::InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: to_ts_point(edit.start_position),
        old_end_position: to_ts_point(edit.old_end_position),
        new_end_position: to_ts_point(edit.new_end_position),
    }
}

/// One query capture clipped to the requested range.
#[derive(Clone, Copy)]
struct Candidate {
    start: usize,
    end: usize,
    pattern: usize,
    specificity: u32,
    name: &'static str,
}

/// Runs the language's highlight query over `range` of `source`.
fn runs_for(lang: Lang, tree: &Tree, source: &str, range: Range<usize>) -> Vec<Run> {
    let compiled = languages::compiled(lang);
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut candidates = Vec::new();
    let mut captures = cursor.captures(&compiled.query, tree.root_node(), source.as_bytes());
    while let Some((matched, index)) = captures.next() {
        let capture = matched.captures()[*index];
        let Some(name) = compiled.names[capture.index as usize] else {
            continue;
        };
        let node = capture.node.byte_range();
        let start = ceil_boundary(source, node.start).max(range.start);
        let end = ceil_boundary(source, node.end).min(range.end);
        if start < end {
            candidates.push(Candidate {
                start,
                end,
                pattern: matched.pattern_index,
                specificity: compiled.specificity[matched.pattern_index],
                name,
            });
        }
    }
    sweep(candidates, range)
}

/// The first char boundary at or after `index`.
fn ceil_boundary(source: &str, mut index: usize) -> usize {
    while index < source.len() && !source.is_char_boundary(index) {
        index += 1;
    }
    index.min(source.len())
}

/// Flattens overlapping captures into gap-free runs over `range`. The
/// smallest capture wins; between captures of the same size the more
/// constrained pattern wins (`(call function: (identifier) @function)` over
/// `(identifier) @variable`), and the later one if they are equally so.
fn sweep(mut candidates: Vec<Candidate>, range: Range<usize>) -> Vec<Run> {
    candidates.sort_by_key(|candidate| candidate.start);
    let mut runs: Vec<Run> = Vec::new();
    let mut active: Vec<Candidate> = Vec::new();
    let mut next = 0;
    let mut position = range.start;
    while position < range.end {
        while next < candidates.len() && candidates[next].start <= position {
            active.push(candidates[next]);
            next += 1;
        }
        active.retain(|candidate| candidate.end > position);

        let mut stop = range.end;
        if let Some(candidate) = candidates.get(next) {
            stop = stop.min(candidate.start);
        }
        for candidate in &active {
            stop = stop.min(candidate.end);
        }
        let name = active
            .iter()
            .min_by_key(|candidate| {
                (
                    candidate.end - candidate.start,
                    Reverse(candidate.specificity),
                    Reverse(candidate.pattern),
                )
            })
            .map(|candidate| candidate.name);

        match runs.last_mut() {
            Some((last, last_name)) if *last_name == name && last.end == position => {
                last.end = stop;
            }
            _ => runs.push((position..stop, name)),
        }
        position = stop;
    }
    runs
}

/// Fold candidates: bracketed nodes plus the indentation-based blocks of
/// Python, Markdown sections and YAML mappings. One range per start line (the
/// editor keeps one), the outermost, ordered by start line.
///
/// The editor keeps the first and last line of a fold visible. Bracketed
/// nodes fold from the opener's line to the closer's line; indentation-based
/// blocks fold from their header line to the line after the block.
fn fold_ranges_of(lang: Lang, tree: &Tree) -> Vec<FoldRange> {
    let mut found: Vec<(usize, usize)> = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let multiline = node.start_position().row < node.end_position().row;
        if multiline {
            if let Some(range) = bracket_fold(node) {
                found.push(range);
            } else if let Some(range) = indent_fold(lang, node) {
                found.push(range);
            }
        }
        if lang == Lang::Python
            && node.kind() == "block"
            && let Some(parent) = node.parent()
        {
            let range = (parent.start_position().row, exclusive_end_row(node));
            if range.1 >= range.0 + 2 {
                found.push(range);
            }
        }

        // Single-line nodes cannot hold a fold, so their subtree is skipped.
        if multiline && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }

    found.sort_by_key(|&(start, end)| (start, Reverse(end)));
    found.dedup_by_key(|&mut (start, _)| start);
    found
        .into_iter()
        .map(|(start, end)| FoldRange::new(start, end))
        .collect()
}

/// A node opened by `{`, `[` or `(` and closed by its match on a later line.
fn bracket_fold(node: Node) -> Option<(usize, usize)> {
    let count = node.child_count();
    if count < 2 {
        return None;
    }
    let first = node.child(0)?;
    let last = node.child(count - 1)?;
    if first.is_named() || last.is_named() || last.is_missing() {
        return None;
    }
    let closes = matches!(
        (first.kind(), last.kind()),
        ("{", "}") | ("[", "]") | ("(", ")")
    );
    let (start, end) = (first.start_position().row, last.start_position().row);
    (closes && end > start).then_some((start, end))
}

fn indent_fold(lang: Lang, node: Node) -> Option<(usize, usize)> {
    let kind = node.kind();
    let foldable = match lang {
        Lang::Markdown => kind == "section",
        Lang::Yaml => kind == "block_mapping_pair",
        _ => false,
    };
    let start = node.start_position().row;
    let end = exclusive_end_row(node);
    (foldable && end >= start + 2).then_some((start, end))
}

/// The row after the node: its own end row, unless it ends at column 0 (a
/// trailing newline), in which case that row is already the next one.
fn exclusive_end_row(node: Node) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row
    } else {
        end.row + 1
    }
}

/// Syntax highlighter for gpui-base's editor: incremental tree-sitter parsing
/// with captures resolved through the editor's [`HighlightStyleResolver`].
pub struct TreeSitterHighlighter {
    syntax: Syntax,
    folding: bool,
}

impl TreeSitterHighlighter {
    pub fn new(lang: Lang) -> Self {
        Self {
            syntax: Syntax::new(lang),
            folding: false,
        }
    }

    pub fn lang(&self) -> Lang {
        self.syntax.lang
    }

    fn sync(&mut self, edits: &[InputEdit], text: &Rope) {
        if text.len() > MAX_HIGHLIGHT_BYTES {
            self.syntax.disable(text.len());
        } else {
            self.syntax.reparse(text.to_string(), edits);
        }
    }
}

impl InputHighlighter for TreeSitterHighlighter {
    fn language(&self) -> SharedString {
        self.syntax.lang.id().into()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.folding = folding;
        self.sync(edit.as_slice(), text);
    }

    fn update_batch(
        &mut self,
        edits: &[(InputEdit, Rope)],
        folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        let Some((_, text)) = edits.last() else {
            return;
        };
        self.folding = folding;
        // Every edit is applied to the old tree, then the final text is
        // parsed once.
        let edits: Vec<InputEdit> = edits.iter().map(|(edit, _)| *edit).collect();
        self.sync(&edits, text);
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.syntax
            .runs(range)
            .into_iter()
            .map(|(range, name)| {
                let style = name.and_then(|name| resolver.style(name));
                (range, style.unwrap_or_default())
            })
            .collect()
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        if self.folding {
            self.syntax.fold_ranges()
        } else {
            Vec::new()
        }
    }
}

/// Builds the editor's highlighter for a language id, alias or extension.
pub fn highlighter_factory() -> InputHighlighterFactory {
    Rc::new(|lang| {
        Lang::from_id(lang)
            .map(|lang| Box::new(TreeSitterHighlighter::new(lang)) as Box<dyn InputHighlighter>)
    })
}

/// Ordered, non-overlapping semantic spans over `text` (byte ranges); the
/// unstyled gaps are not reported. Text over [`MAX_HIGHLIGHT_BYTES`] has none.
pub fn highlight(lang: Lang, text: &str) -> Vec<(Range<usize>, &'static str)> {
    thread_local! {
        static PARSERS: RefCell<HashMap<Lang, Parser>> = RefCell::default();
    }
    if text.is_empty() || text.len() > MAX_HIGHLIGHT_BYTES {
        return Vec::new();
    }
    let tree = PARSERS.with_borrow_mut(|parsers| {
        parsers
            .entry(lang)
            .or_insert_with(|| {
                let mut parser = Parser::new();
                let _ = parser.set_language(&lang.ts_language());
                parser
            })
            .parse(text, None)
    });
    let Some(tree) = tree else {
        return Vec::new();
    };
    runs_for(lang, &tree, text, 0..text.len())
        .into_iter()
        .filter_map(|(range, name)| name.map(|name| (range, name)))
        .collect()
}

/// Styles a Markdown code block; the shape `TextView::shared_code_block_highlighter` takes.
pub type CodeBlockHighlighter =
    Arc<dyn Fn(&gpui_base::text::CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync>;

/// Highlights a Markdown code block by the language in its info string
/// (`rust`, `ts {1,3}`, `rust,no_run`); unknown languages stay plain.
pub fn code_block_highlighter(palette: Arc<SyntaxPalette>) -> CodeBlockHighlighter {
    Arc::new(move |block| {
        let Some(lang) = block.lang().and_then(|info| {
            let id = info
                .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                .next()?;
            Lang::from_id(id)
        }) else {
            return Vec::new();
        };
        highlight(lang, &block.code())
            .into_iter()
            .filter_map(|(range, name)| palette.style(name).map(|style| (range, style)))
            .collect()
    })
}
