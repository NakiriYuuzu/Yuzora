//! Language table: ids, detection, grammars and compiled highlight queries.
//!
//! Everything about a language is one row of [`SPECS`]. Adding one is a
//! [`Lang`] variant, its row (in the same place), and a sample in the tests.

use std::sync::OnceLock;

use tree_sitter::{Language, Query};
use tree_sitter_c::HIGHLIGHT_QUERY as C_QUERY;
use tree_sitter_html::HIGHLIGHTS_QUERY as HTML_QUERY;
use tree_sitter_javascript::{HIGHLIGHT_QUERY as JS, JSX_HIGHLIGHT_QUERY as JSX};
use tree_sitter_language::LanguageFn;
use tree_sitter_typescript::HIGHLIGHTS_QUERY as TS;

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
    CSharp,
    Xml,
    C,
    Cpp,
    Java,
    Kotlin,
    Swift,
    Ruby,
    Php,
    Lua,
    Zig,
    Scala,
    Dart,
    Elixir,
    PowerShell,
    Dockerfile,
    Make,
    Ini,
    Properties,
    Diff,
    Svelte,
    Nix,
    Proto,
    CMake,
    R,
    Erlang,
    Clojure,
    Perl,
    Batch,
}

/// One language.
struct Spec {
    lang: Lang,
    /// The editor's language id.
    id: &'static str,
    /// Display name.
    name: &'static str,
    /// Short badge for the editor header.
    badge: &'static str,
    /// The badge's colour: a theme colour's name (`theme()[name]`), so it
    /// reads on light and dark themes alike. As the files tree's icons.
    badge_color: &'static str,
    /// Other names [`Lang::from_id`] takes: aliases, Markdown fence names and
    /// file extensions, in lowercase.
    aliases: &'static [&'static str],
    /// Whole file names, in lowercase, that are this language whatever their
    /// extension. One ending in `*` is a prefix (`dockerfile.*`).
    files: &'static [&'static str],
    grammar: LanguageFn,
    /// Highlight query sources, most complete first. A candidate that fails
    /// to compile against the grammar is skipped for the next one.
    queries: fn() -> Vec<String>,
}

/// A query that only adds to another language's goes after it, or the base
/// constructs (strings, comments) lose their colour.
fn join(parts: &[&str]) -> String {
    parts.join("\n")
}

/// In `Lang` order: a language's row is `SPECS[lang as usize]`.
const SPECS: &[Spec] = &[
    Spec {
        lang: Lang::Rust,
        id: "rust",
        name: "Rust",
        badge: "RS",
        badge_color: "ansi3",
        aliases: &["rs"],
        files: &[],
        grammar: tree_sitter_rust::LANGUAGE,
        queries: || vec![tree_sitter_rust::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::TypeScript,
        id: "typescript",
        name: "TypeScript",
        badge: "TS",
        badge_color: "ansi4",
        aliases: &["ts", "mts", "cts"],
        files: &[],
        grammar: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        queries: || vec![join(&[JS, TS]), TS.into()],
    },
    Spec {
        lang: Lang::Tsx,
        id: "tsx",
        name: "TypeScript JSX",
        badge: "TS",
        badge_color: "ansi4",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_typescript::LANGUAGE_TSX,
        queries: || vec![join(&[JS, JSX, TS]), join(&[JS, TS]), TS.into()],
    },
    Spec {
        lang: Lang::JavaScript,
        id: "javascript",
        name: "JavaScript",
        badge: "JS",
        badge_color: "ansi11",
        aliases: &["js", "mjs", "cjs", "jsx"],
        files: &[],
        grammar: tree_sitter_javascript::LANGUAGE,
        queries: || vec![join(&[JS, JSX]), JS.into()],
    },
    Spec {
        lang: Lang::Json,
        id: "json",
        name: "JSON",
        badge: "{}",
        badge_color: "ansi11",
        aliases: &["jsonc", "json5"],
        files: &[],
        grammar: tree_sitter_json::LANGUAGE,
        queries: || vec![tree_sitter_json::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Toml,
        id: "toml",
        name: "TOML",
        badge: "TOML",
        badge_color: "ansi6",
        aliases: &[],
        files: &["cargo.lock", "pipfile"],
        grammar: tree_sitter_toml_ng::LANGUAGE,
        // The upstream pattern captures the whole `pair` as a property (spaces
        // and `=` included); the key is what it means to capture.
        queries: || {
            vec![tree_sitter_toml_ng::HIGHLIGHTS_QUERY.replace(
                "(pair\n  (bare_key)) @property",
                "(pair\n  (bare_key) @property)",
            )]
        },
    },
    Spec {
        lang: Lang::Yaml,
        id: "yaml",
        name: "YAML",
        badge: "YML",
        badge_color: "ansi6",
        aliases: &["yml"],
        files: &[],
        grammar: tree_sitter_yaml::LANGUAGE,
        queries: || vec![tree_sitter_yaml::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Markdown,
        id: "markdown",
        name: "Markdown",
        badge: "M↓",
        badge_color: "ansi12",
        aliases: &["md", "mdx"],
        files: &[],
        grammar: tree_sitter_md::LANGUAGE,
        queries: || vec![tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into()],
    },
    Spec {
        lang: Lang::Python,
        id: "python",
        name: "Python",
        badge: "PY",
        badge_color: "ansi4",
        aliases: &["py", "pyi"],
        files: &[],
        grammar: tree_sitter_python::LANGUAGE,
        queries: || vec![tree_sitter_python::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Go,
        id: "go",
        name: "Go",
        badge: "GO",
        badge_color: "ansi14",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_go::LANGUAGE,
        queries: || vec![tree_sitter_go::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Sql,
        id: "sql",
        name: "SQL",
        badge: "SQL",
        badge_color: "ansi13",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_sequel::LANGUAGE,
        // The upstream query spells digits Lua-style (`%d`, `\.`); as a regex that
        // never matches, so every number would stay a string.
        queries: || {
            vec![
                tree_sitter_sequel::HIGHLIGHTS_QUERY
                    .replace("%d", "[0-9]")
                    .replace("\\.", "\\\\."),
            ]
        },
    },
    Spec {
        lang: Lang::Bash,
        id: "bash",
        name: "Shell",
        badge: "SH",
        badge_color: "ansi2",
        // .env files are shell assignments, as Helix and Zed read them.
        aliases: &["sh", "zsh", "shell", "env", "dotenv"],
        files: &[
            ".bashrc",
            ".zshrc",
            ".profile",
            ".bash_profile",
            ".zprofile",
            ".envrc",
            ".env",
            ".env.*",
        ],
        grammar: tree_sitter_bash::LANGUAGE,
        queries: || vec![tree_sitter_bash::HIGHLIGHT_QUERY.into()],
    },
    Spec {
        lang: Lang::Css,
        id: "css",
        name: "CSS",
        badge: "CSS",
        badge_color: "ansi13",
        aliases: &["scss"],
        files: &[],
        grammar: tree_sitter_css::LANGUAGE,
        queries: || vec![tree_sitter_css::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Html,
        id: "html",
        name: "HTML",
        badge: "HTML",
        badge_color: "ansi1",
        // ASP.NET pages read as HTML, an approximation: the markup around the
        // server blocks (`<% %>`, Razor's `@{ }`) is coloured, the blocks are not.
        aliases: &[
            "htm", "xhtml", "aspx", "ascx", "master", "asmx", "ashx", "asax", "cshtml", "vbhtml",
            "razor",
        ],
        files: &[],
        grammar: tree_sitter_html::LANGUAGE,
        queries: || vec![HTML_QUERY.into()],
    },
    Spec {
        lang: Lang::CSharp,
        id: "csharp",
        name: "C#",
        badge: "C#",
        badge_color: "ansi13",
        aliases: &["cs", "c#", "csx"],
        files: &[],
        grammar: tree_sitter_c_sharp::LANGUAGE,
        queries: || vec![tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Xml,
        id: "xml",
        name: "XML",
        badge: "XML",
        badge_color: "ansi1",
        aliases: &[
            "svg", "plist", "xaml", "csproj", "vbproj", "fsproj", "vcxproj", "props", "targets",
            "resx", "nuspec", "config", "sitemap", "xsd", "xsl", "xslt", "wsdl", "manifest",
            "ruleset", "pubxml", "settings", "dbml", "edmx", "rss", "atom",
        ],
        files: &[],
        grammar: tree_sitter_xml::LANGUAGE_XML,
        queries: || vec![tree_sitter_xml::XML_HIGHLIGHT_QUERY.into()],
    },
    Spec {
        lang: Lang::C,
        id: "c",
        name: "C",
        badge: "C",
        badge_color: "ansi4",
        aliases: &["h"],
        files: &[],
        grammar: tree_sitter_c::LANGUAGE,
        queries: || vec![C_QUERY.into()],
    },
    Spec {
        lang: Lang::Cpp,
        id: "cpp",
        name: "C++",
        badge: "C++",
        badge_color: "ansi4",
        aliases: &["c++", "cc", "cxx", "hpp", "hh", "hxx", "h++"],
        files: &[],
        grammar: tree_sitter_cpp::LANGUAGE,
        queries: || {
            let cpp = tree_sitter_cpp::HIGHLIGHT_QUERY;
            vec![join(&[C_QUERY, cpp]), cpp.into()]
        },
    },
    Spec {
        lang: Lang::Java,
        id: "java",
        name: "Java",
        badge: "JAVA",
        badge_color: "ansi3",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_java::LANGUAGE,
        queries: || vec![tree_sitter_java::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Kotlin,
        id: "kotlin",
        name: "Kotlin",
        badge: "KT",
        badge_color: "ansi13",
        aliases: &["kt", "kts"],
        files: &[],
        grammar: tree_sitter_kotlin_sg::LANGUAGE,
        queries: || vec![tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Swift,
        id: "swift",
        name: "Swift",
        badge: "SWIFT",
        badge_color: "ansi3",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_swift::LANGUAGE,
        queries: || vec![tree_sitter_swift::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Ruby,
        id: "ruby",
        name: "Ruby",
        badge: "RB",
        badge_color: "ansi1",
        aliases: &["rb", "rake", "gemspec", "ru"],
        files: &["gemfile", "rakefile", "podfile", "vagrantfile", "brewfile"],
        grammar: tree_sitter_ruby::LANGUAGE,
        queries: || vec![tree_sitter_ruby::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Php,
        id: "php",
        name: "PHP",
        badge: "PHP",
        badge_color: "ansi12",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_php::LANGUAGE_PHP,
        queries: || vec![tree_sitter_php::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Lua,
        id: "lua",
        name: "Lua",
        badge: "LUA",
        badge_color: "ansi4",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_lua::LANGUAGE,
        queries: || vec![tree_sitter_lua::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Zig,
        id: "zig",
        name: "Zig",
        badge: "ZIG",
        badge_color: "ansi3",
        aliases: &["zon"],
        files: &[],
        grammar: tree_sitter_zig::LANGUAGE,
        queries: || vec![tree_sitter_zig::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Scala,
        id: "scala",
        name: "Scala",
        badge: "SCALA",
        badge_color: "ansi1",
        aliases: &["sc", "sbt"],
        files: &[],
        grammar: tree_sitter_scala::LANGUAGE,
        queries: || vec![tree_sitter_scala::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Dart,
        id: "dart",
        name: "Dart",
        badge: "DART",
        badge_color: "ansi6",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_dart_orchard::LANGUAGE,
        queries: || vec![tree_sitter_dart_orchard::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Elixir,
        id: "elixir",
        name: "Elixir",
        badge: "EX",
        badge_color: "ansi13",
        aliases: &["ex", "exs"],
        files: &[],
        grammar: tree_sitter_elixir::LANGUAGE,
        queries: || vec![tree_sitter_elixir::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::PowerShell,
        id: "powershell",
        name: "PowerShell",
        badge: "PS1",
        badge_color: "ansi4",
        aliases: &["ps1", "psm1", "psd1", "pwsh"],
        files: &[],
        grammar: tree_sitter_powershell::LANGUAGE,
        queries: || vec![tree_sitter_powershell::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Dockerfile,
        id: "dockerfile",
        name: "Dockerfile",
        badge: "DOCK",
        badge_color: "ansi4",
        aliases: &["docker", "containerfile"],
        files: &[
            "dockerfile",
            "containerfile",
            "dockerfile.*",
            "containerfile.*",
        ],
        grammar: tree_sitter_containerfile::LANGUAGE,
        queries: || vec![tree_sitter_containerfile::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Make,
        id: "make",
        name: "Makefile",
        badge: "MAKE",
        badge_color: "ansi2",
        aliases: &["makefile", "mk", "mak"],
        files: &["makefile", "gnumakefile"],
        grammar: tree_sitter_make::LANGUAGE,
        queries: || vec![tree_sitter_make::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Ini,
        id: "ini",
        name: "INI",
        badge: "INI",
        badge_color: "ansi6",
        aliases: &["cfg"],
        files: &[".editorconfig", ".gitconfig", ".gitmodules"],
        grammar: tree_sitter_ini::LANGUAGE,
        queries: || vec![tree_sitter_ini::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Properties,
        id: "properties",
        name: "Properties",
        badge: "PROP",
        badge_color: "ansi6",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_properties::LANGUAGE,
        queries: || vec![tree_sitter_properties::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Diff,
        id: "diff",
        name: "Diff",
        badge: "DIFF",
        badge_color: "ansi2",
        aliases: &["patch"],
        files: &[],
        grammar: tree_sitter_diff::LANGUAGE,
        queries: || vec![tree_sitter_diff::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Svelte,
        id: "svelte",
        name: "Svelte",
        badge: "SVLT",
        badge_color: "ansi3",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_svelte_ng::LANGUAGE,
        // Written for Neovim, where it inherits the HTML query.
        queries: || {
            let svelte = tree_sitter_svelte_ng::HIGHLIGHTS_QUERY;
            vec![join(&[HTML_QUERY, svelte]), svelte.into()]
        },
    },
    Spec {
        lang: Lang::Nix,
        id: "nix",
        name: "Nix",
        badge: "NIX",
        badge_color: "ansi4",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_nix::LANGUAGE,
        queries: || vec![tree_sitter_nix::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Proto,
        id: "proto",
        name: "Protocol Buffers",
        badge: "PROTO",
        badge_color: "ansi6",
        aliases: &["protobuf"],
        files: &[],
        grammar: tree_sitter_proto::LANGUAGE,
        queries: || vec![tree_sitter_proto::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::CMake,
        id: "cmake",
        name: "CMake",
        badge: "CMAKE",
        badge_color: "ansi2",
        aliases: &[],
        files: &["cmakelists.txt"],
        grammar: tree_sitter_cmake::LANGUAGE,
        queries: || vec![tree_sitter_cmake::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::R,
        id: "r",
        name: "R",
        badge: "R",
        badge_color: "ansi4",
        aliases: &[],
        files: &[],
        grammar: tree_sitter_r::LANGUAGE,
        queries: || vec![tree_sitter_r::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Erlang,
        id: "erlang",
        name: "Erlang",
        badge: "ERL",
        badge_color: "ansi1",
        aliases: &["erl", "hrl"],
        files: &["rebar.config"],
        grammar: tree_sitter_erlang::LANGUAGE,
        queries: || vec![tree_sitter_erlang::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Clojure,
        id: "clojure",
        name: "Clojure",
        badge: "CLJ",
        badge_color: "ansi2",
        aliases: &["clj", "cljs", "cljc", "edn"],
        files: &[],
        grammar: tree_sitter_clojure_orchard::LANGUAGE,
        queries: || vec![tree_sitter_clojure_orchard::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Perl,
        id: "perl",
        name: "Perl",
        badge: "PL",
        badge_color: "ansi4",
        aliases: &["pl", "pm"],
        files: &[],
        grammar: ts_parser_perl::LANGUAGE,
        queries: || vec![ts_parser_perl::HIGHLIGHTS_QUERY.into()],
    },
    Spec {
        lang: Lang::Batch,
        id: "batch",
        name: "Batch",
        badge: "BAT",
        badge_color: "ansi2",
        aliases: &["bat", "cmd"],
        files: &[],
        grammar: tree_sitter_batch::LANGUAGE,
        queries: || vec![tree_sitter_batch::HIGHLIGHTS_QUERY.into()],
    },
];

const COUNT: usize = SPECS.len();

const ALL: [Lang; COUNT] = {
    let mut all = [Lang::Rust; COUNT];
    let mut i = 0;
    while i < COUNT {
        assert!(SPECS[i].lang as usize == i, "SPECS must be in Lang order");
        all[i] = SPECS[i].lang;
        i += 1;
    }
    all
};

impl Lang {
    pub fn all() -> &'static [Lang] {
        &ALL
    }

    fn spec(self) -> &'static Spec {
        &SPECS[self as usize]
    }

    pub fn id(self) -> &'static str {
        self.spec().id
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// Short badge for the editor header.
    pub fn badge(self) -> &'static str {
        self.spec().badge
    }

    /// The badge's colour: a theme colour's name (`theme()[name]`), so it
    /// reads on light and dark themes alike. As the files tree's icons.
    pub fn badge_color(self) -> &'static str {
        self.spec().badge_color
    }

    /// Resolves a language id, alias or file extension (case-insensitive).
    pub fn from_id(s: &str) -> Option<Lang> {
        let s = s.trim().to_ascii_lowercase();
        SPECS
            .iter()
            .find(|spec| spec.id == s || spec.aliases.contains(&s.as_str()))
            .map(|spec| spec.lang)
    }

    /// Picks a language from a file name or path; unknown names give `None`.
    /// A whole file name wins over the extension (`rebar.config` is Erlang),
    /// and the extension over a name prefix (`Dockerfile.json` is JSON).
    pub fn detect(path: &str) -> Option<Lang> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let name = name.to_ascii_lowercase();
        let named = |prefix: bool| {
            SPECS
                .iter()
                .find(|spec| {
                    spec.files.iter().any(|file| match file.strip_suffix('*') {
                        Some(start) => prefix && name.starts_with(start),
                        None => !prefix && *file == name,
                    })
                })
                .map(|spec| spec.lang)
        };
        named(false)
            .or_else(|| match name.rfind('.') {
                // A leading dot alone (`.gitignore`) is not an extension.
                Some(dot) if dot > 0 => Lang::from_id(&name[dot + 1..]),
                _ => None,
            })
            .or_else(|| named(true))
    }

    pub fn ts_language(self) -> Language {
        self.spec().grammar.into()
    }

    /// Which of the language's query candidates compiled: `Some(0)` is the
    /// preferred query, higher is a fallback, `None` means nothing compiled and
    /// the language has no highlighting.
    pub fn query_level(self) -> Option<usize> {
        compiled(self).level
    }

    /// Every name the language answers to: its id, aliases and file names.
    #[cfg(test)]
    pub(super) fn names(self) -> impl Iterator<Item = &'static str> {
        let spec = self.spec();
        std::iter::once(spec.id).chain(spec.aliases.iter().chain(spec.files).copied())
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
    static CELLS: [OnceLock<Compiled>; COUNT] = [const { OnceLock::new() }; COUNT];
    CELLS[lang as usize].get_or_init(|| compile(lang))
}

fn compile(lang: Lang) -> Compiled {
    let language = lang.ts_language();
    let mut built = None;
    for (level, source) in (lang.spec().queries)().into_iter().enumerate() {
        let source = lua_match_as_regex(&source);
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

/// Queries written for Neovim filter with `#lua-match?`, which tree-sitter
/// does not evaluate: the pattern then matches every node (every Zig
/// identifier a type). Spells each as `#match?` with its Lua pattern as a
/// regex: `%u` is `[[:upper:]]`, `%.` is `\.`, a lone `-` is `*?`.
fn lua_match_as_regex(source: &str) -> String {
    const LUA: &str = "#lua-match?";
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(LUA) {
        out.push_str(&rest[..at]);
        out.push_str("#match?");
        rest = &rest[at + LUA.len()..];
        // The pattern is the predicate's string argument.
        let Some(open) = rest.find('"') else {
            break;
        };
        out.push_str(&rest[..=open]);
        rest = &rest[open + 1..];
        let mut in_set = false;
        let mut end = rest.len();
        let mut chars = rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    end = i;
                    break;
                }
                // A query string escape, kept as it is.
                '\\' => {
                    out.push(c);
                    out.extend(chars.next().map(|(_, next)| next));
                }
                '%' => match chars.next().map(|(_, next)| next) {
                    Some(class) => match lua_class(class) {
                        Some(class) if in_set => out.push_str(&class),
                        Some(class) => out.push_str(&format!("[{class}]")),
                        // An escaped character; `\\` in a query string is one backslash.
                        None if class.is_ascii_punctuation() && class != '%' => {
                            out.push_str("\\\\");
                            out.push(class);
                        }
                        None => out.push(class),
                    },
                    None => {}
                },
                '[' if !in_set => {
                    in_set = true;
                    out.push(c);
                }
                ']' if in_set => {
                    in_set = false;
                    out.push(c);
                }
                '-' if !in_set => out.push_str("*?"),
                '{' | '}' | '|' if !in_set => {
                    out.push_str("\\\\");
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A Lua character class (`%a`, `%D`) as a POSIX class for a bracket
/// expression.
fn lua_class(class: char) -> Option<String> {
    let name = match class.to_ascii_lowercase() {
        'a' => "alpha",
        'c' => "cntrl",
        'd' => "digit",
        'g' => "graph",
        'l' => "lower",
        'p' => "punct",
        's' => "space",
        'u' => "upper",
        'w' => "alnum",
        'x' => "xdigit",
        _ => return None,
    };
    let not = if class.is_ascii_uppercase() { "^" } else { "" };
    Some(format!("[:{not}{name}:]"))
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
