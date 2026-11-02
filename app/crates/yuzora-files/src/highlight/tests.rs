use gpui::HighlightStyle;
use gpui_base::input::{
    FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Point, Rope,
};

use super::*;

fn spans(lang: Lang, text: &str) -> Vec<(String, &'static str)> {
    highlight(lang, text)
        .into_iter()
        .map(|(range, name)| (text[range].to_string(), name))
        .collect()
}

fn has(spans: &[(String, &'static str)], text: &str, name: &str) -> bool {
    spans.iter().any(|(t, n)| t == text && *n == name)
}

fn point_at(text: &str, byte: usize) -> Point {
    let before = &text[..byte];
    let row = before.matches('\n').count();
    let column = byte - before.rfind('\n').map_or(0, |index| index + 1);
    Point::new(row, column)
}

/// Replaces `start..end` of `old` with `insert`, returning the edit and the new text.
fn edit_of(old: &str, start: usize, end: usize, insert: &str) -> (InputEdit, String) {
    let new = format!("{}{}{}", &old[..start], insert, &old[end..]);
    let new_end = start + insert.len();
    let edit = InputEdit {
        start_byte: start,
        old_end_byte: end,
        new_end_byte: new_end,
        start_position: point_at(old, start),
        old_end_position: point_at(old, end),
        new_end_position: point_at(&new, new_end),
    };
    (edit, new)
}

fn highlighter(lang: Lang, text: &str) -> TreeSitterHighlighter {
    let mut highlighter = TreeSitterHighlighter::new(lang);
    highlighter.syntax.reparse(text.to_string(), &[]);
    highlighter
}

fn folds(lang: Lang, text: &str) -> Vec<(usize, usize)> {
    let mut highlighter = highlighter(lang, text);
    highlighter.folding = true;
    highlighter
        .fold_ranges(&Rope::new())
        .into_iter()
        .map(|fold| (fold.start_line, fold.end_line))
        .collect()
}

#[test]
fn every_language_compiles_and_highlights_a_sample() {
    // (language, sample, expected (text, semantic name) pairs)
    type Sample = (Lang, &'static str, &'static [(&'static str, &'static str)]);
    let samples: &[Sample] = &[
        (
            Lang::Rust,
            "fn main() { let s = \"a\"; }",
            &[("fn", "keyword"), ("\"a\"", "string")],
        ),
        (
            Lang::TypeScript,
            "const x: number = 1;",
            &[("const", "keyword"), ("number", "type.builtin")],
        ),
        (
            Lang::Tsx,
            "const a = <div className=\"x\">hi</div>;",
            &[("const", "keyword"), ("div", "tag")],
        ),
        (
            Lang::JavaScript,
            "function foo() { return 1; }",
            &[
                ("function", "keyword"),
                ("foo", "function"),
                ("1", "number"),
            ],
        ),
        (
            Lang::Json,
            "{\"a\": \"b\", \"n\": 1, \"t\": true}",
            &[("\"a\"", "property"), ("\"b\"", "string"), ("1", "number")],
        ),
        (
            Lang::Toml,
            "[package]\nname = \"x\"\nn = 1\n",
            &[("name", "property"), ("\"x\"", "string"), ("1", "number")],
        ),
        (
            Lang::Yaml,
            "key: value\nn: 1\n# note\n",
            &[("key", "property"), ("# note", "comment")],
        ),
        (Lang::Markdown, "# Title\n\ntext\n", &[("Title", "heading")]),
        (
            Lang::Python,
            "def f(x):\n    return 1\n",
            &[("def", "keyword"), ("f", "function"), ("1", "number")],
        ),
        (
            Lang::Go,
            "package main\nfunc main() {}\n",
            &[("package", "keyword"), ("func", "keyword")],
        ),
        (
            Lang::Sql,
            "SELECT 1",
            &[("SELECT", "keyword"), ("1", "number")],
        ),
        (Lang::Bash, "echo \"hi\"", &[("\"hi\"", "string")]),
        (
            Lang::Css,
            "a { color: red; }",
            &[("a", "tag"), ("color", "property")],
        ),
        (
            Lang::Html,
            "<div class=\"a\">hi</div>",
            &[("div", "tag"), ("class", "attribute")],
        ),
    ];
    assert_eq!(samples.len(), Lang::all().len());

    for (lang, sample, expected) in samples {
        assert!(
            lang.query_level().is_some(),
            "{lang:?}: no highlight query compiled"
        );
        let found = spans(*lang, sample);
        assert!(!found.is_empty(), "{lang:?}: no spans");
        for (text, name) in *expected {
            assert!(
                has(&found, text, name),
                "{lang:?}: expected {text:?} as {name}, got {found:?}"
            );
        }
    }
}

#[test]
fn the_preferred_query_compiles_for_every_language() {
    for lang in Lang::all() {
        assert_eq!(lang.query_level(), Some(0), "{lang:?} needed a fallback");
    }
}

#[test]
fn the_more_constrained_pattern_wins_wherever_it_sits_in_the_query() {
    // `(identifier) @variable` comes first in the JavaScript query; the
    // function pattern after it wins for the same node.
    let found = spans(
        Lang::JavaScript,
        "function foo() {}\nfoo();\nconst MAX = 1;",
    );
    assert!(!has(&found, "foo", "variable"));
    assert!(
        found
            .iter()
            .filter(|(t, n)| t == "foo" && *n == "function")
            .count()
            >= 2
    );
    assert!(has(&found, "MAX", "constant"));
    // The JSON key pattern comes before `(string) @string` and still wins.
    let found = spans(Lang::Json, "{\"a\": \"b\"}");
    assert!(has(&found, "\"a\"", "property") && has(&found, "\"b\"", "string"));
    // Go's method pattern comes before the bare `(field_identifier) @property`.
    let found = spans(
        Lang::Go,
        "package p\nfunc (t T) M() {}\nfunc main() { x.Run() }",
    );
    assert!(has(&found, "M", "function.method"), "{found:?}");
    assert!(has(&found, "main", "function"), "{found:?}");
    assert!(has(&found, "Run", "function.method"), "{found:?}");
    // CSS custom properties beat the plain property pattern.
    let found = spans(Lang::Css, "a { --c: 1; color: red }");
    assert!(has(&found, "--c", "variable"), "{found:?}");
    assert!(has(&found, "color", "property"), "{found:?}");
}

#[test]
fn pattern_specificity_counts_nodes_and_predicates() {
    use super::languages::specificity;
    assert_eq!(specificity("(identifier) @variable"), 1);
    assert_eq!(specificity("(pair key: (_) @key)"), 2);
    assert_eq!(specificity("[\"a\" \"b\" (c)] @keyword"), 1);
    assert_eq!(specificity("\"fn\" @keyword"), 1);
    // Group, node and predicate; strings inside the predicate are not nodes.
    assert_eq!(
        specificity("((identifier) @c\n (#match? @c \"^[A-Z](\"))"),
        4
    );
    assert_eq!(specificity("; (comment (x))\n(a) @a"), 1);
}

#[test]
fn toml_keys_are_properties_and_table_names_are_types() {
    let text = "[package]\nname = \"x\"\nserde.version = 1\n";
    let found = spans(Lang::Toml, text);
    assert!(has(&found, "package", "type"), "{found:?}");
    assert!(has(&found, "name", "property"), "{found:?}");
    assert!(has(&found, "version", "property"), "{found:?}");
    // The whitespace and `=` around a key are not part of it.
    assert!(
        !found
            .iter()
            .any(|(t, n)| t.trim().is_empty() && *n == "property")
    );
    assert!(has(&found, "=", "operator"), "{found:?}");
}

#[test]
fn sql_numbers_are_numbers_and_quoted_text_stays_a_string() {
    let found = spans(Lang::Sql, "SELECT 42, 2.5, 'a1' FROM t");
    assert!(has(&found, "42", "number"), "{found:?}");
    assert!(has(&found, "2.5", "number"), "{found:?}");
    assert!(has(&found, "'a1'", "string"), "{found:?}");
}

#[test]
fn typescript_adds_types_on_top_of_the_javascript_query() {
    let found = spans(
        Lang::TypeScript,
        "function f(a: string): Foo { return a; }\nlet n: number;",
    );
    assert!(has(&found, "function", "keyword"));
    assert!(has(&found, "f", "function"));
    assert!(has(&found, "string", "type.builtin"));
    assert!(has(&found, "Foo", "type"));
}

#[test]
fn detect_picks_a_language_from_the_file_name() {
    assert_eq!(Lang::detect("a.tsx"), Some(Lang::Tsx));
    assert_eq!(Lang::detect("x.d.ts"), Some(Lang::TypeScript));
    assert_eq!(Lang::detect("src/lib.rs"), Some(Lang::Rust));
    assert_eq!(Lang::detect("C:\\proj\\Main.PY"), Some(Lang::Python));
    assert_eq!(Lang::detect("Cargo.lock"), Some(Lang::Toml));
    assert_eq!(Lang::detect("/home/u/.zshrc"), Some(Lang::Bash));
    assert_eq!(Lang::detect(".envrc"), Some(Lang::Bash));
    assert_eq!(Lang::detect("README.md"), Some(Lang::Markdown));
    assert_eq!(Lang::detect("view.jsx"), Some(Lang::JavaScript));
    assert_eq!(Lang::detect("Dockerfile"), None);
    assert_eq!(Lang::detect("README"), None);
    assert_eq!(Lang::detect(".gitignore"), None);
    assert_eq!(Lang::detect("foo.unknown"), None);
}

#[test]
fn from_id_accepts_ids_aliases_and_extensions() {
    for lang in Lang::all() {
        assert_eq!(Lang::from_id(lang.id()), Some(*lang));
        assert!(!lang.name().is_empty() && !lang.badge().is_empty());
        assert!(lang.badge_color() <= 0xffffff);
    }
    for (alias, lang) in [
        ("RS", Lang::Rust),
        ("mts", Lang::TypeScript),
        ("cjs", Lang::JavaScript),
        ("jsx", Lang::JavaScript),
        ("json5", Lang::Json),
        ("yml", Lang::Yaml),
        ("mdx", Lang::Markdown),
        ("pyi", Lang::Python),
        ("zsh", Lang::Bash),
        ("shell", Lang::Bash),
        ("scss", Lang::Css),
        ("xhtml", Lang::Html),
    ] {
        assert_eq!(Lang::from_id(alias), Some(lang), "{alias}");
    }
    assert_eq!(Lang::from_id("cobol"), None);
    assert_eq!(Lang::Tsx.badge(), "TS");
    assert_eq!(Lang::Bash.name(), "Shell");
}

#[test]
fn capture_names_map_to_semantic_names() {
    for (capture, expected) in [
        ("keyword", Some("keyword")),
        ("keyword.control.return", Some("keyword")),
        ("keyword.operator", Some("operator")),
        ("function.method", Some("function.method")),
        ("function.macro", Some("function")),
        ("function.call", Some("function")),
        ("type.builtin", Some("type.builtin")),
        ("text.title", Some("heading")),
        ("markup.heading.2", Some("heading")),
        ("text.emphasis", Some("emphasis")),
        ("markup.bold", Some("strong")),
        ("text.uri", Some("link")),
        ("markup.link.url", Some("link")),
        ("text.literal", Some("literal")),
        ("markup.raw.block", Some("literal")),
        ("field", Some("property")),
        ("method", Some("function.method")),
        ("escape", Some("string.escape")),
        ("delimiter", Some("punctuation.delimiter")),
        ("constant.numeric", Some("number")),
        ("float", Some("number")),
        ("character", Some("string")),
        ("string.special.key", Some("property")),
        ("comment.documentation", Some("comment")),
        ("punctuation.special", Some("punctuation")),
        ("none", None),
        ("spell", None),
        ("nonsense", None),
        ("", None),
    ] {
        assert_eq!(semantic_name(capture), expected, "{capture:?}");
    }
}

#[test]
fn styles_cover_the_range_without_gaps() {
    let text = "fn main() {\n    let s = \"héllo → 世界\"; // done\n}\n";
    let highlighter = highlighter(Lang::Rust, text);
    let palette = SyntaxPalette::one_dark();
    for range in [
        0..text.len(),
        0..3,
        5..text.len() - 2,
        12..text.len(),
        7..7,
        text.len()..text.len() + 4,
    ] {
        let runs = highlighter.styles(&range, &palette);
        let clamped = range.start.min(text.len())..range.end.min(text.len());
        if clamped.is_empty() {
            assert!(runs.is_empty(), "{range:?}");
            continue;
        }
        assert_eq!(runs.first().unwrap().0.start, clamped.start);
        assert_eq!(runs.last().unwrap().0.end, clamped.end);
        for pair in runs.windows(2) {
            assert_eq!(pair[0].0.end, pair[1].0.start, "{range:?}: {runs:?}");
        }
        for (run, _) in &runs {
            assert!(run.start < run.end);
            assert!(text.is_char_boundary(run.start) && text.is_char_boundary(run.end));
        }
    }
}

#[test]
fn styles_carry_the_resolved_colours() {
    let text = "fn x() {}";
    let runs = highlighter(Lang::Rust, text).styles(&(0..text.len()), &SyntaxPalette::one_dark());
    let keyword = SyntaxPalette::one_dark().style("keyword").unwrap();
    assert_eq!(runs[0], (0..2, keyword));
    // The space after `fn` is unstyled.
    assert_eq!(runs[1], (2..3, HighlightStyle::default()));
}

#[test]
fn an_empty_or_unparsed_highlighter_degrades_quietly() {
    let fresh = TreeSitterHighlighter::new(Lang::Rust);
    assert!(
        fresh
            .styles(&(0..10), &SyntaxPalette::one_dark())
            .is_empty()
    );

    let mut highlighter = highlighter(Lang::Json, "{}");
    highlighter.syntax.reparse(String::new(), &[]);
    assert!(
        highlighter
            .styles(&(0..5), &SyntaxPalette::one_dark())
            .is_empty()
    );
}

#[test]
fn incremental_edits_match_a_fresh_parse() {
    type Case = (Lang, &'static str, &'static [(usize, usize, &'static str)]);
    let cases: &[Case] = &[
        (
            Lang::Rust,
            "fn main() {\n    let x = 1;\n}\n",
            &[
                (12, 12, "    // héllo\n"),
                (3, 7, "start"),
                (0, 0, "pub "),
                (0, 4, ""),
            ],
        ),
        (
            Lang::TypeScript,
            "const a: number = 1;\nfunction f() { return a; }\n",
            &[(6, 7, "bb"), (21, 21, "type T = string;\n"), (0, 6, "let")],
        ),
        (
            Lang::Json,
            "{\"a\": [1, 2, 3], \"b\": null}",
            &[(6, 6, "\"x\", "), (1, 4, "\"zz\""), (24, 24, ", \"c\": {}")],
        ),
        (
            Lang::Markdown,
            "# One\n\ntext\n\n## Two\n",
            &[(5, 5, " more"), (0, 0, "#"), (13, 13, "\n- item\n")],
        ),
        (
            Lang::Python,
            "def f(x):\n    return x\n",
            &[(9, 9, "\n    y = 2"), (0, 3, "async def")],
        ),
    ];
    for (lang, text, edits) in cases {
        let mut inc = Syntax::new(*lang);
        let mut current = text.to_string();
        inc.reparse(current.clone(), &[]);
        for &(start, end, insert) in *edits {
            let (edit, next) = edit_of(&current, start, end, insert);
            inc.reparse(next.clone(), &[edit]);
            current = next;

            let mut fresh = Syntax::new(*lang);
            fresh.reparse(current.clone(), &[]);
            assert_eq!(
                inc.runs(&(0..current.len())),
                fresh.runs(&(0..current.len())),
                "{lang:?} after editing {start}..{end} -> {insert:?}"
            );
            assert_eq!(inc.fold_ranges(), fresh.fold_ranges());
        }
    }
}

#[test]
fn several_edits_reparse_once_and_match_a_fresh_parse() {
    let mut syntax = Syntax::new(Lang::Rust);
    let text = "fn a() {}\nfn b() {}\n";
    syntax.reparse(text.to_string(), &[]);

    let (first, after_first) = edit_of(text, 3, 4, "alpha");
    let (second, after_second) = edit_of(
        &after_first,
        after_first.len() - 7,
        after_first.len() - 6,
        "bravo",
    );
    syntax.reparse(after_second.clone(), &[first, second]);

    let mut fresh = Syntax::new(Lang::Rust);
    fresh.reparse(after_second.clone(), &[]);
    assert_eq!(
        syntax.runs(&(0..after_second.len())),
        fresh.runs(&(0..after_second.len()))
    );
}

#[test]
fn edits_that_do_not_fit_fall_back_to_a_full_parse() {
    let mut syntax = Syntax::new(Lang::Json);
    syntax.reparse("{\"a\": 1}".to_string(), &[]);
    // Claims a 1-byte insert, but the text grew by far more.
    let (edit, _) = edit_of("{\"a\": 1}", 1, 1, "x");
    let text = "{\"a\": 1, \"b\": [true, false]}".to_string();
    syntax.reparse(text.clone(), &[edit]);

    let mut fresh = Syntax::new(Lang::Json);
    fresh.reparse(text.clone(), &[]);
    assert_eq!(syntax.runs(&(0..text.len())), fresh.runs(&(0..text.len())));
}

#[test]
fn oversized_sources_are_not_parsed() {
    let text = " ".repeat(MAX_HIGHLIGHT_BYTES + 1);
    assert!(highlight(Lang::Json, &text).is_empty());

    let mut highlighter = TreeSitterHighlighter::new(Lang::Json);
    highlighter.syntax.reparse(text.clone(), &[]);
    assert!(highlighter.syntax.tree.is_none());
    assert!(highlighter.syntax.source.is_empty());
    let runs = highlighter.styles(&(10..100), &SyntaxPalette::one_dark());
    assert_eq!(runs, vec![(10..100, HighlightStyle::default())]);
}

#[test]
fn rust_functions_and_json_objects_fold() {
    assert_eq!(
        folds(
            Lang::Rust,
            "fn main() {\n    let x = 1;\n    let y = 2;\n}\n"
        ),
        vec![(0, 3)]
    );
    // Single-line bodies do not fold; nested blocks fold separately.
    assert_eq!(
        folds(
            Lang::Rust,
            "fn a() {}\nfn b() {\n    if x {\n        y();\n    }\n}\n"
        ),
        vec![(1, 5), (2, 4)]
    );
    assert_eq!(
        folds(
            Lang::Json,
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {}\n}\n"
        ),
        vec![(0, 6), (1, 4)]
    );
    assert_eq!(
        folds(Lang::Json, "{\"a\": 1}"),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn indentation_based_blocks_fold_from_their_header() {
    // Python folds from the `def` line to the line after the body.
    assert_eq!(
        folds(Lang::Python, "def f():\n    a = 1\n    b = 2\nx = 3\n"),
        vec![(0, 3)]
    );
    // Markdown sections end at the next heading of the same or higher level.
    assert_eq!(
        folds(Lang::Markdown, "# One\n\ntext\n\n# Two\n\nmore\n"),
        vec![(0, 4), (4, 7)]
    );
    // YAML mappings with nested values.
    assert_eq!(
        folds(Lang::Yaml, "a:\n  b: 1\n  c: 2\nd: 3\n"),
        vec![(0, 3)]
    );
}

#[test]
fn folding_is_off_unless_the_editor_asks_for_it() {
    let highlighter = highlighter(Lang::Rust, "fn main() {\n\n}\n");
    assert!(highlighter.fold_ranges(&Rope::new()).is_empty());
    assert_eq!(folds(Lang::Rust, "fn main() {\n\n}\n"), vec![(0, 2)]);
    assert_eq!(FoldRange::new(0, 2).end_line, 2);
}

#[test]
fn the_factory_builds_highlighters_by_language_id() {
    let factory = highlighter_factory();
    assert_eq!(factory("rs").unwrap().language().to_string(), "rust");
    assert_eq!(factory("TSX").unwrap().language().to_string(), "tsx");
    assert!(factory("cobol").is_none());
}

#[test]
fn palette_resolves_names_by_prefix() {
    let palette = SyntaxPalette::one_dark();
    assert_eq!(palette.name(), "One Dark");

    let color = |name: &str| palette.style(name).and_then(|style| style.color);
    assert_eq!(color("keyword"), Some(parse_hex("#c678dd").unwrap()));
    assert_eq!(
        color("function.method"),
        Some(parse_hex("#61afef").unwrap())
    );
    assert_eq!(color("string.escape"), Some(parse_hex("#56b6c2").unwrap()));
    assert_eq!(color("keyword.control"), color("keyword"));
    assert_eq!(color("property"), Some(parse_hex("#e06c75").unwrap()));
    assert_eq!(color("comment"), Some(parse_hex("#7f848e").unwrap()));
    assert_eq!(
        palette.style("comment").unwrap().font_style,
        Some(gpui::FontStyle::Italic)
    );
    assert_eq!(
        palette.style("heading").unwrap().font_weight,
        Some(gpui::FontWeight::BOLD)
    );
    assert!(palette.style("link").unwrap().underline.is_some());
    // Unclassified text follows the theme foreground.
    assert!(palette.style("variable").is_none());
    assert!(palette.style("punctuation.bracket").is_none());
    assert!(palette.style("no.such.name").is_none());
}

#[test]
fn palette_overrides_replace_colour_and_keep_the_rest() {
    let red = parse_hex("#ff0000").unwrap();
    let blue = parse_hex("#0000ff").unwrap();
    let palette = SyntaxPalette::one_dark().with_overrides([
        ("comment".to_string(), red),
        ("function".to_string(), blue),
        ("variable".to_string(), blue),
        ("function.method".to_string(), red),
    ]);

    let comment = palette.style("comment").unwrap();
    assert_eq!(comment.color, Some(red));
    assert_eq!(comment.font_style, Some(gpui::FontStyle::Italic));
    // Exact name first, then the root segment.
    assert_eq!(palette.style("function.method").unwrap().color, Some(red));
    assert_eq!(palette.style("function.builtin").unwrap().color, Some(blue));
    // An override can colour a name the palette leaves alone.
    assert_eq!(palette.style("variable").unwrap().color, Some(blue));
    // Untouched names keep the palette colour.
    assert_eq!(
        palette.style("string").unwrap().color,
        SyntaxPalette::one_dark().style("string").unwrap().color
    );
}

#[test]
fn parse_hex_reads_rgb_and_rgba() {
    assert_eq!(parse_hex("#c678dd"), Some(gpui::rgb(0xc678dd).into()));
    assert_eq!(parse_hex(" #C678DD "), Some(gpui::rgb(0xc678dd).into()));
    let translucent = parse_hex("#c678dd80").unwrap();
    assert!((translucent.a - 128.0 / 255.0).abs() < 1e-3);
    for bad in [
        "", "#", "c678dd", "#c678d", "#c678ddd", "#gggggg", "#+12345", "#c678dd8",
    ] {
        assert_eq!(parse_hex(bad), None, "{bad:?}");
    }
}

#[test]
fn markdown_code_blocks_are_highlighted_by_their_info_string() {
    let highlighter = code_block_highlighter(Arc::new(SyntaxPalette::one_dark()));
    let block = |lang: Option<&str>| gpui_base::text::CodeBlock::from_code("fn main() {}", lang);

    let styled = highlighter(&block(Some("rust")));
    assert!(styled.iter().any(|(range, _)| *range == (0..2)));
    assert_eq!(highlighter(&block(Some("rust,no_run"))), styled);
    assert_eq!(highlighter(&block(Some("RS {1}"))), styled);
    assert!(highlighter(&block(Some("brainfuck"))).is_empty());
    assert!(highlighter(&block(None)).is_empty());
}

#[test]
fn the_resolver_and_highlight_api_are_thread_safe() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SyntaxPalette>();
    let handles: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| highlight(Lang::Rust, "fn main() {}").len()))
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap() > 0);
    }
}
