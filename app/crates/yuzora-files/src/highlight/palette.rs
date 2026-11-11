//! Semantic names for tree-sitter captures, and the colour palette over them.

use std::collections::HashMap;

use gpui::{FontStyle, FontWeight, HighlightStyle, Hsla, UnderlineStyle, px};
use gpui_base::input::HighlightStyleResolver;

/// The semantic names a highlighter may report.
const NAMES: &[&str] = &[
    "keyword",
    "function",
    "function.method",
    "function.builtin",
    "type",
    "type.builtin",
    "constructor",
    "string",
    "string.escape",
    "string.special",
    "number",
    "boolean",
    "constant",
    "constant.builtin",
    "comment",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "property",
    "attribute",
    "tag",
    "operator",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "label",
    "namespace",
    "module",
    "embedded",
    "heading",
    "emphasis",
    "strong",
    "link",
    "literal",
];

/// Capture names that differ from the semantic set. `None` means the capture
/// is deliberately ignored.
const ALIASES: &[(&str, Option<&str>)] = &[
    ("text.title", Some("heading")),
    ("markup.heading", Some("heading")),
    ("text.emphasis", Some("emphasis")),
    ("markup.italic", Some("emphasis")),
    ("text.strong", Some("strong")),
    ("markup.bold", Some("strong")),
    ("text.uri", Some("link")),
    ("text.reference", Some("link")),
    ("markup.link", Some("link")),
    ("text.literal", Some("literal")),
    ("markup.raw", Some("literal")),
    ("field", Some("property")),
    ("method", Some("function.method")),
    ("escape", Some("string.escape")),
    ("delimiter", Some("punctuation.delimiter")),
    ("constant.numeric", Some("number")),
    ("float", Some("number")),
    ("character", Some("string")),
    ("parameter", Some("variable.parameter")),
    // JSON object keys read as properties, not as special strings.
    ("string.special.key", Some("property")),
    ("keyword.operator", Some("operator")),
    // Vim-style keyword families used by older queries.
    ("type.qualifier", Some("keyword")),
    ("storageclass", Some("keyword")),
    ("conditional", Some("keyword")),
    ("repeat", Some("keyword")),
    ("include", Some("keyword")),
    ("exception", Some("keyword")),
    // Markers that exist only to switch highlighting off.
    ("none", None),
    ("spell", None),
    ("nospell", None),
];

/// Maps a tree-sitter capture name to a semantic name by longest prefix: the
/// name itself, then each dotted prefix. Unknown captures give `None`.
pub fn semantic_name(capture: &str) -> Option<&'static str> {
    let mut candidate = capture;
    loop {
        if let Some((_, mapped)) = ALIASES.iter().find(|(alias, _)| *alias == candidate) {
            return *mapped;
        }
        if let Some(name) = NAMES.iter().copied().find(|name| *name == candidate) {
            return Some(name);
        }
        candidate = candidate.rsplit_once('.')?.0;
    }
}

/// A named set of colours over the semantic names, usable as the editor's and
/// the Markdown code blocks' style resolver.
#[derive(Clone, Debug)]
pub struct SyntaxPalette {
    name: String,
    styles: HashMap<&'static str, HighlightStyle>,
    overrides: HashMap<String, Hsla>,
}

fn color(hex: u32) -> HighlightStyle {
    HighlightStyle {
        color: Some(gpui::rgb(hex).into()),
        ..Default::default()
    }
}

impl SyntaxPalette {
    pub fn one_dark() -> Self {
        let italic = |hex| HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..color(hex)
        };
        let bold = |hex| HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..color(hex)
        };
        let styles = HashMap::from([
            ("keyword", color(0xc678dd)),
            ("function", color(0x61afef)),
            ("function.method", color(0x61afef)),
            ("function.builtin", color(0x56b6c2)),
            ("type", color(0xe5c07b)),
            ("type.builtin", color(0xe5c07b)),
            ("constructor", color(0xe5c07b)),
            ("string", color(0x98c379)),
            ("string.escape", color(0x56b6c2)),
            ("string.special", color(0x56b6c2)),
            ("number", color(0xd19a66)),
            ("boolean", color(0xd19a66)),
            ("constant", color(0xd19a66)),
            ("constant.builtin", color(0xd19a66)),
            ("comment", italic(0x7f848e)),
            ("variable.builtin", color(0xe06c75)),
            ("variable.parameter", color(0xe06c75)),
            ("property", color(0xe06c75)),
            ("attribute", color(0xd19a66)),
            ("tag", color(0xe06c75)),
            ("operator", color(0x56b6c2)),
            ("label", color(0xe06c75)),
            ("namespace", color(0xe5c07b)),
            ("module", color(0xe5c07b)),
            ("heading", bold(0xe06c75)),
            (
                "emphasis",
                HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
            ),
            (
                "strong",
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ),
            (
                "link",
                HighlightStyle {
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    ..color(0x61afef)
                },
            ),
            ("literal", color(0x98c379)),
        ]);
        Self {
            name: "One Dark".into(),
            styles,
            overrides: HashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The names it gives a color of its own.
    #[cfg(test)]
    pub(crate) fn colored(&self) -> Vec<&'static str> {
        self.styles
            .iter()
            .filter(|(_, s)| s.color.is_some())
            .map(|(k, _)| *k)
            .collect()
    }

    /// Replaces colours by semantic name. An override keeps the palette's
    /// weight, slant and underline for that name.
    pub fn with_overrides(mut self, overrides: impl IntoIterator<Item = (String, Hsla)>) -> Self {
        self.overrides.extend(overrides);
        self
    }
}

/// `name`, then each dotted prefix of it: `a.b.c`, `a.b`, `a`.
fn prefixes(name: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(name), |name| {
        name.rsplit_once('.').map(|(head, _)| head)
    })
}

impl HighlightStyleResolver for SyntaxPalette {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let base = prefixes(name).find_map(|prefix| self.styles.get(prefix).copied());
        let over = prefixes(name).find_map(|prefix| self.overrides.get(prefix).copied());
        match (base, over) {
            (base, None) => base,
            (base, Some(color)) => Some(HighlightStyle {
                color: Some(color),
                ..base.unwrap_or_default()
            }),
        }
    }
}

/// Parses `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Option<Hsla> {
    let digits = s.trim().strip_prefix('#')?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some(gpui::rgb(value).into()),
        8 => Some(gpui::rgba(value).into()),
        _ => None,
    }
}
