//! The syntax of a diff's code, the editor's own (`yuzora_files::highlight`).
//!
//! A side is parsed whole, as the editor parses the file, so a comment or a
//! string that begins outside a hunk still colours the hunk's lines. The spans
//! name what a piece of code is (`keyword`, `string`), not a colour: the line
//! element ([`crate::line_view`]) styles them with the editor's palette under
//! the theme's colours.

use std::ops::Range;
use std::sync::OnceLock;

use gpui_base::input::HighlightStyleResolver;
use yuzora_files::highlight::{self, Lang, SyntaxPalette};

/// A range of a text and the semantic name it is styled by.
pub type Span = (Range<usize>, &'static str);

/// The syntax of one side of a diff.
pub struct Syntax {
    /// In order and apart, as ranges of the side's text (UTF-8, lossy).
    spans: Vec<Span>,
    /// Where each line starts in that text.
    starts: Vec<usize>,
}

/// Whether the editor's palette styles a name at all. Variables and punctuation are
/// plain text there, so they are no span here.
fn styled(name: &str) -> bool {
    static PALETTE: OnceLock<SyntaxPalette> = OnceLock::new();
    PALETTE.get_or_init(SyntaxPalette::one_dark).style(name).is_some()
}

impl Syntax {
    /// The syntax of `side`, the text of a file named `path`. None where the
    /// editor shows plain text: a name of no language it knows, a text past
    /// its limit, or one with nothing to style.
    pub fn of(path: &str, side: &[u8]) -> Option<Syntax> {
        let lang = Lang::detect(path)?;
        let text = String::from_utf8_lossy(side);
        let spans: Vec<Span> = highlight::highlight(lang, &text).into_iter().filter(|(_, name)| styled(name)).collect();
        if spans.is_empty() {
            return None;
        }
        let starts = std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1)).collect();
        Some(Syntax { spans, starts })
    }

    /// The spans of line `k` (0-based), as ranges of the line as the file has it
    /// (lossy UTF-8, tabs as tabs). A span may run into the line's end; past the
    /// last line there are none.
    pub fn line(&self, k: usize) -> Vec<Span> {
        let Some(&start) = self.starts.get(k) else { return Vec::new() };
        let end = self.starts.get(k + 1).copied().unwrap_or(usize::MAX);
        let first = self.spans.partition_point(|(r, _)| r.end <= start);
        self.spans[first..]
            .iter()
            .take_while(|(r, _)| r.start < end)
            .map(|(r, name)| (r.start.max(start) - start..r.end.min(end) - start, *name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(spans: &[Span], name: &str) -> Vec<Range<usize>> {
        spans.iter().filter(|(_, n)| *n == name).map(|(r, _)| r.clone()).collect()
    }

    #[test]
    fn a_lines_spans_are_ranges_of_the_line_as_the_file_has_it() {
        // The comment begins on the second line and ends on the third; both begin with a tab.
        let s = Syntax::of("f.rs", b"fn f() {\n\tlet a = 1; /* one\n\ttwo */ let b = 2;\n}\n").unwrap();
        let second = s.line(1);
        assert_eq!(named(&second, "keyword"), [1..4], "`let` after the tab, the tab one byte");
        assert_eq!(named(&second, "comment"), [12..19], "to the line's end, its `\\n` included");
        let third = s.line(2);
        assert_eq!(named(&third, "comment"), [0..7], "from the line's start: it began on the line before");
        assert_eq!(named(&third, "keyword"), [8..11]);
        assert_eq!(s.line(4), []);
        assert_eq!(s.line(9), [], "past the last line");
    }

    #[test]
    fn a_byte_that_is_not_utf8_is_one_character_where_the_spans_are_counted() {
        let s = Syntax::of("t.rs", b"let name = \"caf\xff\"; // note\n").unwrap();
        let line = s.line(0);
        // `\xff` is U+FFFD, three bytes: the string is `"caf\u{fffd}"`, and the comment begins two after it.
        assert_eq!(named(&line, "string"), [11..19]);
        assert_eq!(named(&line, "comment")[0].start, 21);
    }

    #[test]
    fn a_file_of_no_known_language_or_with_nothing_to_style_has_no_syntax() {
        assert!(Syntax::of("notes.txt", b"let a = 1;\n").is_none());
        assert!(Syntax::of("LICENSE", b"let a = 1;\n").is_none());
        assert!(Syntax::of("empty.rs", b"").is_none());
        assert!(Syntax::of("main.rs", b"fn main() {}\n").is_some());
    }

    #[test]
    fn only_what_the_editor_styles_is_a_span() {
        let s = Syntax::of("a.rs", b"let total = count + 1;\n").unwrap();
        let names: Vec<&str> = s.line(0).iter().map(|(_, n)| *n).collect();
        assert!(names.contains(&"keyword") && names.contains(&"constant.builtin"), "{names:?}");
        let palette = SyntaxPalette::one_dark();
        assert!(names.iter().all(|n| palette.style(n).is_some()), "`total` and `count` are plain: {names:?}");
    }
}
