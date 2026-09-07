// M3 Task 2: LSP curated adapter registry

#[derive(Debug, Clone, Copy)]
pub struct Adapter {
    pub id: &'static str,
    pub display: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub install_hint: &'static str,
}

pub struct LanguageAdapters {
    pub language: &'static str,
    pub default_id: &'static str,
    pub options: &'static [Adapter],
}

static TYPESCRIPT: &[Adapter] = &[
    Adapter {
        id: "vtsls",
        display: "vtsls",
        command: "vtsls",
        args: &["--stdio"],
        install_hint: "npm i -g @vtsls/language-server",
    },
    Adapter {
        id: "typescript-language-server",
        display: "typescript-language-server",
        command: "typescript-language-server",
        args: &["--stdio"],
        install_hint: "npm i -g typescript-language-server typescript",
    },
];

static PYTHON: &[Adapter] = &[
    Adapter {
        id: "pyright",
        display: "pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
        install_hint: "npm i -g pyright",
    },
    Adapter {
        id: "pylsp",
        display: "pylsp",
        command: "pylsp",
        args: &[],
        install_hint: "pip install python-lsp-server",
    },
];

static RUST: &[Adapter] = &[Adapter {
    id: "rust-analyzer",
    display: "rust-analyzer",
    command: "rust-analyzer",
    args: &[],
    install_hint: "rustup component add rust-analyzer",
}];

static MARKDOWN: &[Adapter] = &[
    Adapter {
        id: "marksman",
        display: "marksman",
        command: "marksman",
        args: &["server"],
        install_hint: "https://github.com/artempyanykh/marksman/releases",
    },
    Adapter {
        id: "markdown-oxide",
        display: "markdown-oxide",
        command: "markdown-oxide",
        args: &[],
        install_hint: "cargo install --locked --git https://github.com/Feel-ix-343/markdown-oxide.git markdown-oxide",
    },
];

static ALL: &[LanguageAdapters] = &[
    LanguageAdapters {
        language: "typescript",
        default_id: "vtsls",
        options: TYPESCRIPT,
    },
    LanguageAdapters {
        language: "python",
        default_id: "pyright",
        options: PYTHON,
    },
    LanguageAdapters {
        language: "rust",
        default_id: "rust-analyzer",
        options: RUST,
    },
    LanguageAdapters {
        language: "markdown",
        default_id: "marksman",
        options: MARKDOWN,
    },
];

pub fn all() -> &'static [LanguageAdapters] {
    ALL
}

pub fn adapters_for(language: &str) -> Option<&'static LanguageAdapters> {
    ALL.iter().find(|l| l.language == language)
}

pub fn adapter(language: &str, id: &str) -> Option<&'static Adapter> {
    adapters_for(language).and_then(|l| l.options.iter().find(|a| a.id == id))
}

pub fn language_for_path(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" => Some("typescript"),
        "py" | "pyi" => Some("python"),
        "rs" => Some("rust"),
        "md" | "markdown" => Some("markdown"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_for_path_maps_typescript_family() {
        assert_eq!(language_for_path("a.ts"), Some("typescript"));
        assert_eq!(language_for_path("a.tsx"), Some("typescript"));
        assert_eq!(language_for_path("a.js"), Some("typescript"));
        assert_eq!(language_for_path("a.jsx"), Some("typescript"));
        assert_eq!(language_for_path("a.mts"), Some("typescript"));
        assert_eq!(language_for_path("a.cts"), Some("typescript"));
    }

    #[test]
    fn language_for_path_maps_python_rust_markdown() {
        assert_eq!(language_for_path("a.py"), Some("python"));
        assert_eq!(language_for_path("a.pyi"), Some("python"));
        assert_eq!(language_for_path("a.rs"), Some("rust"));
        assert_eq!(language_for_path("a.md"), Some("markdown"));
        assert_eq!(language_for_path("a.markdown"), Some("markdown"));
    }

    #[test]
    fn language_for_path_is_case_insensitive() {
        assert_eq!(language_for_path("A.TS"), Some("typescript"));
        assert_eq!(language_for_path("README.MD"), Some("markdown"));
        assert_eq!(language_for_path("Main.RS"), Some("rust"));
    }

    #[test]
    fn language_for_path_returns_none_for_unknown_or_missing_ext() {
        assert_eq!(language_for_path("notes.txt"), None);
        assert_eq!(language_for_path("Makefile"), None);
        assert_eq!(language_for_path("no_ext"), None);
    }

    #[test]
    fn all_covers_four_languages() {
        let langs: Vec<&str> = all().iter().map(|l| l.language).collect();
        assert_eq!(langs, vec!["typescript", "python", "rust", "markdown"]);
    }

    #[test]
    fn adapters_for_returns_defaults() {
        assert_eq!(adapters_for("python").unwrap().default_id, "pyright");
        assert_eq!(adapters_for("typescript").unwrap().default_id, "vtsls");
        assert_eq!(adapters_for("rust").unwrap().default_id, "rust-analyzer");
        assert_eq!(adapters_for("markdown").unwrap().default_id, "marksman");
    }

    #[test]
    fn adapters_for_unknown_language_is_none() {
        assert!(adapters_for("go").is_none());
    }

    #[test]
    fn adapter_lookup_hits_and_misses() {
        assert!(adapter("typescript", "typescript-language-server").is_some());
        assert!(adapter("typescript", "vtsls").is_some());
        assert!(adapter("python", "pylsp").is_some());
        assert!(adapter("markdown", "markdown-oxide").is_some());
        assert!(adapter("rust", "pyright").is_none());
        assert!(adapter("typescript", "nope").is_none());
        assert!(adapter("go", "vtsls").is_none());
    }

    #[test]
    fn adapter_command_and_args_match_curated() {
        let py = adapter("python", "pyright").unwrap();
        assert_eq!(py.command, "pyright-langserver");
        assert_eq!(py.args, &["--stdio"]);

        let vtsls = adapter("typescript", "vtsls").unwrap();
        assert_eq!(vtsls.command, "vtsls");
        assert_eq!(vtsls.args, &["--stdio"]);

        let ra = adapter("rust", "rust-analyzer").unwrap();
        assert_eq!(ra.command, "rust-analyzer");
        assert!(ra.args.is_empty());

        let mk = adapter("markdown", "marksman").unwrap();
        assert_eq!(mk.command, "marksman");
        assert_eq!(mk.args, &["server"]);
    }
}
