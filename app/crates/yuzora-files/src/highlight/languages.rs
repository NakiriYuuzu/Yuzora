//! Language table: ids, detection, grammars and compiled highlight queries.

use std::sync::OnceLock;

use tree_sitter::{Language, Query};

use super::palette::semantic_name;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Json,
    Toml,
    Yaml,
    Markdown,
    Python,
    Go,
    Sql,
    Bash,
    Css,
    Html,
}

const ALL: [Lang; 14] = [
    Lang::Rust,
    Lang::TypeScript,
    Lang::Tsx,
    Lang::JavaScript,
    Lang::Json,
    Lang::Toml,
    Lang::Yaml,
    Lang::Markdown,
    Lang::Python,
    Lang::Go,
    Lang::Sql,
    Lang::Bash,
    Lang::Css,
    Lang::Html,
];

impl Lang {
    pub fn all() -> &'static [Lang] {
        &ALL
    }

    pub fn id(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::JavaScript => "javascript",
            Lang::Json => "json",
            Lang::Toml => "toml",
            Lang::Yaml => "yaml",
            Lang::Markdown => "markdown",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::Sql => "sql",
            Lang::Bash => "bash",
            Lang::Css => "css",
            Lang::Html => "html",
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::TypeScript => "TypeScript",
            Lang::Tsx => "TypeScript JSX",
            Lang::JavaScript => "JavaScript",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Yaml => "YAML",
            Lang::Markdown => "Markdown",
            Lang::Python => "Python",
            Lang::Go => "Go",
            Lang::Sql => "SQL",
            Lang::Bash => "Shell",
            Lang::Css => "CSS",
            Lang::Html => "HTML",
        }
    }

    /// Short badge for the editor header.
    pub fn badge(self) -> &'static str {
        match self {
            Lang::Rust => "RS",
            Lang::TypeScript | Lang::Tsx => "TS",
            Lang::JavaScript => "JS",
            Lang::Json => "{}",
            Lang::Toml => "TOML",
            Lang::Yaml => "YML",
            Lang::Markdown => "M↓",
            Lang::Python => "PY",
            Lang::Go => "GO",
            Lang::Sql => "SQL",
            Lang::Bash => "SH",
            Lang::Css => "CSS",
            Lang::Html => "HTML",
        }
    }

    /// The badge's colour: a theme colour's name (`theme()[name]`), so it
    /// reads on light and dark themes alike. As the files tree's icons.
    pub fn badge_color(self) -> &'static str {
        match self {
            Lang::Rust => "ansi3",
            Lang::TypeScript | Lang::Tsx | Lang::Python => "ansi4",
            Lang::JavaScript | Lang::Json => "ansi11",
            Lang::Toml | Lang::Yaml => "ansi6",
            Lang::Go => "ansi14",
            Lang::Markdown => "ansi12",
            Lang::Sql | Lang::Css => "ansi13",
            Lang::Bash => "ansi2",
            Lang::Html => "ansi1",
        }
    }

    /// Resolves a language id, alias or file extension (case-insensitive).
    pub fn from_id(s: &str) -> Option<Lang> {
        let lang = match s.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Lang::Rust,
            "typescript" | "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "javascript" | "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "json" | "jsonc" | "json5" => Lang::Json,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "markdown" | "md" | "mdx" => Lang::Markdown,
            "python" | "py" | "pyi" => Lang::Python,
            "go" => Lang::Go,
            "sql" => Lang::Sql,
            "bash" | "sh" | "zsh" | "shell" => Lang::Bash,
            "css" | "scss" => Lang::Css,
            "html" | "htm" | "xhtml" => Lang::Html,
            _ => return None,
        };
        Some(lang)
    }

    /// Picks a language from a file name or path; unknown names give `None`.
    pub fn detect(path: &str) -> Option<Lang> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        match name.to_ascii_lowercase().as_str() {
            "cargo.lock" | "pipfile" => return Some(Lang::Toml),
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".zprofile" | ".envrc" => {
                return Some(Lang::Bash);
            }
            _ => {}
        }
        // A leading dot alone (`.gitignore`) is not an extension.
        match name.rfind('.') {
            Some(dot) if dot > 0 => Lang::from_id(&name[dot + 1..]),
            _ => None,
        }
    }

    pub fn ts_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Sql => tree_sitter_sequel::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
        }
    }

    /// Which entry of [`Self::query_candidates`] compiled: `Some(0)` is the
    /// preferred query, higher is a fallback, `None` means nothing compiled
    /// and the language has no highlighting.
    pub fn query_level(self) -> Option<usize> {
        compiled(self).level
    }

    /// Highlight query sources, most complete first. A candidate that fails to
    /// compile against the paired grammar is skipped for the next one.
    fn query_candidates(self) -> Vec<String> {
        use tree_sitter_javascript::{HIGHLIGHT_QUERY as JS, JSX_HIGHLIGHT_QUERY as JSX};
        use tree_sitter_typescript::HIGHLIGHTS_QUERY as TS;
        let join = |parts: &[&str]| parts.join("\n");
        match self {
            Lang::Rust => vec![tree_sitter_rust::HIGHLIGHTS_QUERY.into()],
            // The TypeScript query only adds to the JavaScript one, so it goes after it.
            Lang::TypeScript => vec![join(&[JS, TS]), TS.into()],
            Lang::Tsx => vec![join(&[JS, JSX, TS]), join(&[JS, TS]), TS.into()],
            Lang::JavaScript => vec![join(&[JS, JSX]), JS.into()],
            Lang::Json => vec![tree_sitter_json::HIGHLIGHTS_QUERY.into()],
            // The upstream pattern captures the whole `pair` as a property
            // (spaces and `=` included); the key is what it means to capture.
            Lang::Toml => vec![tree_sitter_toml_ng::HIGHLIGHTS_QUERY.replace(
                "(pair\n  (bare_key)) @property",
                "(pair\n  (bare_key) @property)",
            )],
            Lang::Yaml => vec![tree_sitter_yaml::HIGHLIGHTS_QUERY.into()],
            Lang::Markdown => vec![tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into()],
            Lang::Python => vec![tree_sitter_python::HIGHLIGHTS_QUERY.into()],
            Lang::Go => vec![tree_sitter_go::HIGHLIGHTS_QUERY.into()],
            // The upstream query spells digits Lua-style (`%d`, `\.`); as a
            // regex that never matches, so every number would stay a string.
            Lang::Sql => vec![
                tree_sitter_sequel::HIGHLIGHTS_QUERY
                    .replace("%d", "[0-9]")
                    .replace("\\.", "\\\\."),
            ],
            Lang::Bash => vec![tree_sitter_bash::HIGHLIGHT_QUERY.into()],
            Lang::Css => vec![tree_sitter_css::HIGHLIGHTS_QUERY.into()],
            Lang::Html => vec![tree_sitter_html::HIGHLIGHTS_QUERY.into()],
        }
    }
}

/// A highlight query compiled once per language.
pub(crate) struct Compiled {
    pub query: Query,
    /// Semantic name per capture index; `None` captures are ignored.
    pub names: Vec<Option<&'static str>>,
    /// How constrained each pattern is; see [`specificity`].
    pub specificity: Vec<u32>,
    pub level: Option<usize>,
}

pub(crate) fn compiled(lang: Lang) -> &'static Compiled {
    static CELLS: [OnceLock<Compiled>; 14] = [const { OnceLock::new() }; 14];
    CELLS[lang as usize].get_or_init(|| compile(lang))
}

fn compile(lang: Lang) -> Compiled {
    let language = lang.ts_language();
    let mut built = None;
    for (level, source) in lang.query_candidates().into_iter().enumerate() {
        if let Ok(query) = Query::new(&language, &source) {
            built = Some((query, source, Some(level)));
            break;
        }
    }
    let (query, source, level) = built.unwrap_or_else(|| {
        let empty = Query::new(&language, "").expect("an empty query always compiles");
        (empty, String::new(), None)
    });
    let names = query
        .capture_names()
        .iter()
        .map(|name| semantic_name(name))
        .collect();
    let specificity = (0..query.pattern_count())
        .map(|pattern| {
            source
                .get(query.start_byte_for_pattern(pattern)..query.end_byte_for_pattern(pattern))
                .map_or(0, specificity)
        })
        .collect();
    Compiled {
        query,
        names,
        specificity,
        level,
    }
}

/// Counts what a query pattern constrains: node patterns (an alternation
/// counts as one) and predicates. Upstream queries do not agree on whether the
/// first or the last pattern wins for a node, but they all put the more
/// constrained pattern (`(call function: (identifier) @function)`) where it
/// should beat the bare one (`(identifier) @variable`), so that decides.
pub(super) fn specificity(pattern: &str) -> u32 {
    let bytes = pattern.as_bytes();
    let (mut score, mut alternation) = (0, 0u32);
    // Open parentheses; `true` marks a predicate such as `(#match? ...)`.
    let mut open: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let in_predicate = open.last() == Some(&true);
        match bytes[i] {
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += usize::from(bytes[i] == b'\\') + 1;
                }
                if !in_predicate && alternation == 0 {
                    score += 1;
                }
            }
            b'[' => {
                if !in_predicate && alternation == 0 {
                    score += 1;
                }
                alternation += 1;
            }
            b']' => alternation = alternation.saturating_sub(1),
            b'(' => {
                let predicate =
                    bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'#');
                open.push(predicate);
                if predicate {
                    score += 2;
                } else if alternation == 0 {
                    score += 1;
                }
            }
            b')' => {
                open.pop();
            }
            _ => {}
        }
        i += 1;
    }
    score
}
