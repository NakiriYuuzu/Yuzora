// Pure install-decision layer for managed LSP server installs: route
// classification, GitHub asset URL / unpack routing, integrity, command + bin
// path assembly, and plan binding. No IO — everything here is decidable without
// touching the network or subprocesses, and is unit-tested below. The impure
// execution layer (download / npm / pip / venv) lives in the parent module and
// consumes these via `super`.

use std::path::{Path, PathBuf};

use crate::{lsp_adapters, lsp_config};

// ---- install-route classification (pure) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryServer {
    RustAnalyzer,
    Marksman,
    MarkdownOxide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackKind {
    Gz,
    Bare,
    Zip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallRoute {
    Binary(BinaryServer),
    Npm {
        packages: &'static [&'static str],
        bin: &'static str,
    },
    Pip {
        package: &'static str,
        bin: &'static str,
    },
}

/// Map a curated (language, server_id) to its managed-install route. Err for any
/// pair not in the curated registry.
pub fn route_for(language: &str, server_id: &str) -> Result<InstallRoute, String> {
    let route = match (language, server_id) {
        ("typescript", "vtsls") => InstallRoute::Npm {
            packages: &["@vtsls/language-server"],
            bin: "vtsls",
        },
        ("typescript", "typescript-language-server") => InstallRoute::Npm {
            packages: &["typescript-language-server", "typescript"],
            bin: "typescript-language-server",
        },
        ("python", "pyright") => InstallRoute::Npm {
            packages: &["pyright"],
            bin: "pyright-langserver",
        },
        ("python", "pylsp") => InstallRoute::Pip {
            package: "python-lsp-server",
            bin: "pylsp",
        },
        ("rust", "rust-analyzer") => InstallRoute::Binary(BinaryServer::RustAnalyzer),
        ("markdown", "marksman") => InstallRoute::Binary(BinaryServer::Marksman),
        ("markdown", "markdown-oxide") => InstallRoute::Binary(BinaryServer::MarkdownOxide),
        _ => return Err(format!("no managed install for {server_id} ({language})")),
    };
    Ok(route)
}

/// Resolve the active server for a language: a workspace override (by canonical
/// key, matching the set_server write side) wins, then the global default, then
/// the adapter's curated default. `workspace` is the already-canonicalized key.
pub fn resolve_active(
    cfg: &lsp_config::LspConfig,
    workspace: Option<&str>,
    language: &str,
) -> Option<String> {
    let configured = match workspace {
        // resolve_server already falls back to the global default for the language.
        Some(ws) => lsp_config::resolve_server(cfg, ws, language),
        None => cfg.defaults.get(language).cloned(),
    };
    configured.or_else(|| lsp_adapters::adapters_for(language).map(|a| a.default_id.to_string()))
}

/// Canonicalize a raw workspace path to the key overrides are stored under,
/// mirroring lsp_config set_server's write side (canonicalize, raw fallback).
pub fn canonical_key(workspace: Option<&str>) -> Option<String> {
    workspace.map(|p| lsp_config::canonicalize(p).unwrap_or_else(|| p.to_string()))
}

// ---- binary route: asset URL / unpack / dest (pure) ----

/// Official GitHub release download URL for a binary server on (os, arch), where
/// os is `std::env::consts::OS` and arch is `std::env::consts::ARCH`. Err for an
/// unsupported platform.
pub fn asset_url(server: BinaryServer, os: &str, arch: &str) -> Result<String, String> {
    let unsupported = || format!("unsupported platform {os}/{arch} for {server:?}");
    let url = match server {
        BinaryServer::RustAnalyzer => {
            let base = "https://github.com/rust-lang/rust-analyzer/releases/download/2026-06-29";
            let asset = match (os, arch) {
                ("macos", "aarch64") => "rust-analyzer-aarch64-apple-darwin.gz",
                ("macos", "x86_64") => "rust-analyzer-x86_64-apple-darwin.gz",
                ("linux", "aarch64") => "rust-analyzer-aarch64-unknown-linux-gnu.gz",
                ("linux", "x86_64") => "rust-analyzer-x86_64-unknown-linux-gnu.gz",
                ("windows", "aarch64") => "rust-analyzer-aarch64-pc-windows-msvc.zip",
                ("windows", "x86_64") => "rust-analyzer-x86_64-pc-windows-msvc.zip",
                _ => return Err(unsupported()),
            };
            format!("{base}/{asset}")
        }
        BinaryServer::Marksman => {
            let base = "https://github.com/artempyanykh/marksman/releases/download/2026-02-08";
            // macOS ships a single universal binary (no arch split).
            let asset = match (os, arch) {
                ("macos", _) => "marksman-macos",
                ("linux", "aarch64") => "marksman-linux-arm64",
                ("linux", "x86_64") => "marksman-linux-x64",
                ("windows", _) => "marksman.exe",
                _ => return Err(unsupported()),
            };
            format!("{base}/{asset}")
        }
        BinaryServer::MarkdownOxide => {
            let ver = "v0.25.12";
            let base = "https://github.com/Feel-ix-343/markdown-oxide/releases/download/v0.25.12";
            let asset = match (os, arch) {
                ("macos", "aarch64") => format!("markdown-oxide-{ver}-aarch64-apple-darwin"),
                ("macos", "x86_64") => format!("markdown-oxide-{ver}-x86_64-apple-darwin"),
                ("linux", "aarch64") => format!("markdown-oxide-{ver}-aarch64-unknown-linux-gnu"),
                ("linux", "x86_64") => format!("markdown-oxide-{ver}-x86_64-unknown-linux-gnu"),
                ("windows", "x86_64") => format!("markdown-oxide-{ver}-x86_64-pc-windows-gnu.exe"),
                _ => return Err(unsupported()),
            };
            format!("{base}/{asset}")
        }
    };
    Ok(url)
}

/// Post-download unpack step for a server on an os. rust-analyzer ships `.gz`
/// (macOS/Linux) or `.zip` (Windows); marksman / markdown-oxide are bare binaries.
pub fn unpack_kind(server: BinaryServer, os: &str) -> UnpackKind {
    match server {
        BinaryServer::RustAnalyzer => {
            if os == "windows" {
                UnpackKind::Zip
            } else {
                UnpackKind::Gz
            }
        }
        BinaryServer::Marksman | BinaryServer::MarkdownOxide => UnpackKind::Bare,
    }
}

/// The `which`-resolvable command name a downloaded binary must be saved as.
pub fn binary_command(server: BinaryServer) -> &'static str {
    match server {
        BinaryServer::RustAnalyzer => "rust-analyzer",
        BinaryServer::Marksman => "marksman",
        BinaryServer::MarkdownOxide => "markdown-oxide",
    }
}

/// Where a downloaded binary is written under the servers root (`.exe` on Windows).
pub fn binary_dest(base: &Path, server: BinaryServer, windows: bool) -> PathBuf {
    let name = binary_command(server);
    if windows {
        base.join(format!("{name}.exe"))
    } else {
        base.join(name)
    }
}

/// Sibling temp path for an atomic install: same directory as `dest` so the final
/// `rename` is atomic and never truncates a running binary's inode (F3).
pub fn binary_temp(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

// ---- integrity (pure) ----

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn sha256_matches(bytes: &[u8], expected_hex: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expected_hex)
}

// ---- command / path assembly (pure) ----

/// macOS Gatekeeper quarantine removal, as (program, args).
pub fn quarantine_command(path: &str) -> (&'static str, Vec<String>) {
    (
        "xattr",
        vec![
            "-d".to_string(),
            "com.apple.quarantine".to_string(),
            path.to_string(),
        ],
    )
}

pub fn npm_prefix(base: &Path) -> PathBuf {
    base.join("npm")
}

pub fn npm_install_args(prefix: &Path, packages: &[&str]) -> Vec<String> {
    let mut args = vec![
        "install".to_string(),
        "--prefix".to_string(),
        prefix.to_string_lossy().into_owned(),
    ];
    args.extend(packages.iter().map(|p| p.to_string()));
    args
}

pub fn npm_bin_path(base: &Path, bin: &str, windows: bool) -> PathBuf {
    let dir = npm_prefix(base).join("node_modules").join(".bin");
    if windows {
        // npm shims land as `.cmd` on Windows (matches lsp_service which()).
        dir.join(format!("{bin}.cmd"))
    } else {
        dir.join(bin)
    }
}

pub fn venv_dir(base: &Path) -> PathBuf {
    base.join("pyenv")
}

pub fn venv_args(venv_dir: &Path) -> Vec<String> {
    vec![
        "-m".to_string(),
        "venv".to_string(),
        venv_dir.to_string_lossy().into_owned(),
    ]
}

pub fn venv_bin_path(base: &Path, name: &str, windows: bool) -> PathBuf {
    let venv = venv_dir(base);
    if windows {
        venv.join("Scripts").join(format!("{name}.exe"))
    } else {
        venv.join("bin").join(name)
    }
}

pub fn pip_install_args(package: &str) -> Vec<String> {
    vec!["install".to_string(), package.to_string()]
}

// ---- resolved plan + missing-tool branches (pure) ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Binary(BinaryServer),
    Npm {
        npm: String,
        packages: &'static [&'static str],
        bin: &'static str,
    },
    Pip {
        python: String,
        package: &'static str,
        bin: &'static str,
    },
}

/// Bind a route to the resolved toolchain, or return the guided missing-tool
/// error (T12 surfaces it). `npm` / `python` are the resolved absolute paths, or
/// None when the tool is absent.
pub fn build_plan(
    language: &str,
    route: InstallRoute,
    npm: Option<String>,
    python: Option<String>,
) -> Result<Plan, String> {
    match route {
        InstallRoute::Binary(server) => Ok(Plan::Binary(server)),
        InstallRoute::Npm { packages, bin } => match npm {
            Some(npm) => Ok(Plan::Npm { npm, packages, bin }),
            None => Err(format!(
                "安裝 {language} 的 {bin} 需先安裝 Node.js（npm）。請安裝 Node.js 後重試，或依安裝提示手動安裝。"
            )),
        },
        InstallRoute::Pip { package, bin } => match python {
            Some(python) => Ok(Plan::Pip {
                python,
                package,
                bin,
            }),
            None => Err(format!(
                "安裝 {language} 的 {bin} 需先安裝 Python（python3）。請安裝 Python 後重試，或依安裝提示手動安裝。"
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- route classification: the seven curated adapters ----

    #[test]
    fn route_for_covers_all_seven_curated_adapters() {
        assert_eq!(
            route_for("typescript", "vtsls").unwrap(),
            InstallRoute::Npm {
                packages: &["@vtsls/language-server"],
                bin: "vtsls"
            }
        );
        assert_eq!(
            route_for("typescript", "typescript-language-server").unwrap(),
            InstallRoute::Npm {
                packages: &["typescript-language-server", "typescript"],
                bin: "typescript-language-server"
            }
        );
        assert_eq!(
            route_for("python", "pyright").unwrap(),
            InstallRoute::Npm {
                packages: &["pyright"],
                bin: "pyright-langserver"
            }
        );
        assert_eq!(
            route_for("python", "pylsp").unwrap(),
            InstallRoute::Pip {
                package: "python-lsp-server",
                bin: "pylsp"
            }
        );
        assert_eq!(
            route_for("rust", "rust-analyzer").unwrap(),
            InstallRoute::Binary(BinaryServer::RustAnalyzer)
        );
        assert_eq!(
            route_for("markdown", "marksman").unwrap(),
            InstallRoute::Binary(BinaryServer::Marksman)
        );
        assert_eq!(
            route_for("markdown", "markdown-oxide").unwrap(),
            InstallRoute::Binary(BinaryServer::MarkdownOxide)
        );
    }

    #[test]
    fn route_for_binary_npm_pip_split_is_three_three_one() {
        let all = [
            ("typescript", "vtsls"),
            ("typescript", "typescript-language-server"),
            ("python", "pyright"),
            ("python", "pylsp"),
            ("rust", "rust-analyzer"),
            ("markdown", "marksman"),
            ("markdown", "markdown-oxide"),
        ];
        let (mut b, mut n, mut p) = (0, 0, 0);
        for (l, s) in all {
            match route_for(l, s).unwrap() {
                InstallRoute::Binary(_) => b += 1,
                InstallRoute::Npm { .. } => n += 1,
                InstallRoute::Pip { .. } => p += 1,
            }
        }
        assert_eq!((b, n, p), (3, 3, 1));
    }

    #[test]
    fn every_curated_adapter_has_a_route() {
        // Ties the route table to the adapter registry (source of truth): a new
        // curated adapter without a route fails here.
        for lang in lsp_adapters::all() {
            for opt in lang.options {
                assert!(
                    route_for(lang.language, opt.id).is_ok(),
                    "no managed-install route for {}/{}",
                    lang.language,
                    opt.id
                );
            }
        }
    }

    #[test]
    fn route_for_unknown_pair_is_err() {
        assert!(route_for("go", "gopls").is_err());
        assert!(route_for("rust", "pyright").is_err());
        assert!(route_for("typescript", "nope").is_err());
    }

    // ---- asset URL assembly (macOS arm64/x64 + Windows) ----

    #[test]
    fn asset_url_rust_analyzer_per_platform() {
        let base = "https://github.com/rust-lang/rust-analyzer/releases/download/2026-06-29";
        assert_eq!(
            asset_url(BinaryServer::RustAnalyzer, "macos", "aarch64").unwrap(),
            format!("{base}/rust-analyzer-aarch64-apple-darwin.gz")
        );
        assert_eq!(
            asset_url(BinaryServer::RustAnalyzer, "macos", "x86_64").unwrap(),
            format!("{base}/rust-analyzer-x86_64-apple-darwin.gz")
        );
        // Windows ships a .zip (recorded discrepancy vs the brief's ".gz only").
        assert_eq!(
            asset_url(BinaryServer::RustAnalyzer, "windows", "x86_64").unwrap(),
            format!("{base}/rust-analyzer-x86_64-pc-windows-msvc.zip")
        );
    }

    #[test]
    fn asset_url_marksman_per_platform() {
        let base = "https://github.com/artempyanykh/marksman/releases/download/2026-02-08";
        // macOS ships a single universal binary (no arch split).
        assert_eq!(
            asset_url(BinaryServer::Marksman, "macos", "aarch64").unwrap(),
            format!("{base}/marksman-macos")
        );
        assert_eq!(
            asset_url(BinaryServer::Marksman, "macos", "x86_64").unwrap(),
            format!("{base}/marksman-macos")
        );
        assert_eq!(
            asset_url(BinaryServer::Marksman, "windows", "x86_64").unwrap(),
            format!("{base}/marksman.exe")
        );
    }

    #[test]
    fn asset_url_markdown_oxide_per_platform() {
        let base = "https://github.com/Feel-ix-343/markdown-oxide/releases/download/v0.25.12";
        assert_eq!(
            asset_url(BinaryServer::MarkdownOxide, "macos", "aarch64").unwrap(),
            format!("{base}/markdown-oxide-v0.25.12-aarch64-apple-darwin")
        );
        assert_eq!(
            asset_url(BinaryServer::MarkdownOxide, "macos", "x86_64").unwrap(),
            format!("{base}/markdown-oxide-v0.25.12-x86_64-apple-darwin")
        );
        assert_eq!(
            asset_url(BinaryServer::MarkdownOxide, "windows", "x86_64").unwrap(),
            format!("{base}/markdown-oxide-v0.25.12-x86_64-pc-windows-gnu.exe")
        );
    }

    #[test]
    fn asset_url_unsupported_platform_is_err() {
        assert!(asset_url(BinaryServer::RustAnalyzer, "macos", "riscv64").is_err());
        assert!(asset_url(BinaryServer::Marksman, "freebsd", "x86_64").is_err());
    }

    // ---- unpack routing (gz vs bare vs zip) ----

    #[test]
    fn unpack_kind_routes_gz_bare_zip() {
        assert_eq!(
            unpack_kind(BinaryServer::RustAnalyzer, "macos"),
            UnpackKind::Gz
        );
        assert_eq!(
            unpack_kind(BinaryServer::RustAnalyzer, "linux"),
            UnpackKind::Gz
        );
        assert_eq!(
            unpack_kind(BinaryServer::RustAnalyzer, "windows"),
            UnpackKind::Zip
        );
        assert_eq!(
            unpack_kind(BinaryServer::Marksman, "macos"),
            UnpackKind::Bare
        );
        assert_eq!(
            unpack_kind(BinaryServer::Marksman, "windows"),
            UnpackKind::Bare
        );
        assert_eq!(
            unpack_kind(BinaryServer::MarkdownOxide, "macos"),
            UnpackKind::Bare
        );
        assert_eq!(
            unpack_kind(BinaryServer::MarkdownOxide, "windows"),
            UnpackKind::Bare
        );
    }

    #[test]
    fn binary_command_and_dest_layout() {
        assert_eq!(binary_command(BinaryServer::RustAnalyzer), "rust-analyzer");
        assert_eq!(binary_command(BinaryServer::Marksman), "marksman");
        assert_eq!(
            binary_command(BinaryServer::MarkdownOxide),
            "markdown-oxide"
        );
        let base = PathBuf::from("/home/u/.yuzora/servers");
        assert_eq!(
            binary_dest(&base, BinaryServer::RustAnalyzer, false),
            base.join("rust-analyzer")
        );
        assert_eq!(
            binary_dest(&base, BinaryServer::RustAnalyzer, true),
            base.join("rust-analyzer.exe")
        );
    }

    #[test]
    fn binary_temp_appends_tmp_in_same_dir() {
        // W6A-F3: temp sits beside dest so the rename is atomic (same filesystem).
        assert_eq!(
            binary_temp(Path::new("/home/u/.yuzora/servers/rust-analyzer")),
            PathBuf::from("/home/u/.yuzora/servers/rust-analyzer.tmp")
        );
        // A `.exe` dest (Windows) keeps its parent dir too.
        assert_eq!(
            binary_temp(Path::new("/x/servers/marksman.exe")),
            PathBuf::from("/x/servers/marksman.exe.tmp")
        );
    }

    // ---- SHA256 (given bytes + hex) ----

    #[test]
    fn sha256_hex_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_matches_case_insensitive_and_rejects_mismatch() {
        let d = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(sha256_matches(b"abc", d));
        assert!(sha256_matches(b"abc", &d.to_uppercase()));
        assert!(!sha256_matches(b"abc", "00"));
        assert!(!sha256_matches(b"abcd", d));
    }

    // ---- quarantine command ----

    #[test]
    fn quarantine_command_shape() {
        let (prog, args) = quarantine_command("/x/rust-analyzer");
        assert_eq!(prog, "xattr");
        assert_eq!(args, vec!["-d", "com.apple.quarantine", "/x/rust-analyzer"]);
    }

    // ---- npm command + private-prefix bin path ----

    #[test]
    fn npm_prefix_and_bin_path_match_service_layout() {
        let base = PathBuf::from("/home/u/.yuzora/servers");
        assert_eq!(npm_prefix(&base), base.join("npm"));
        // Matches lsp_service::server_bin_dirs_from's npm landing spot.
        assert_eq!(
            npm_bin_path(&base, "vtsls", false),
            base.join("npm")
                .join("node_modules")
                .join(".bin")
                .join("vtsls")
        );
        // Windows npm shims land as `.cmd`.
        assert_eq!(
            npm_bin_path(&base, "vtsls", true),
            base.join("npm")
                .join("node_modules")
                .join(".bin")
                .join("vtsls.cmd")
        );
    }

    #[test]
    fn npm_install_args_shape() {
        let prefix = PathBuf::from("/home/u/.yuzora/servers/npm");
        assert_eq!(
            npm_install_args(&prefix, &["typescript-language-server", "typescript"]),
            vec![
                "install".to_string(),
                "--prefix".to_string(),
                prefix.to_string_lossy().into_owned(),
                "typescript-language-server".to_string(),
                "typescript".to_string(),
            ]
        );
    }

    // ---- venv + pip command + bin path (macOS bin vs Windows Scripts) ----

    #[test]
    fn venv_dir_args_and_bin_paths_two_shapes() {
        let base = PathBuf::from("/home/u/.yuzora/servers");
        assert_eq!(venv_dir(&base), base.join("pyenv"));
        assert_eq!(
            venv_args(&venv_dir(&base)),
            vec![
                "-m".to_string(),
                "venv".to_string(),
                base.join("pyenv").to_string_lossy().into_owned(),
            ]
        );
        // unix: pyenv/bin/*, windows: pyenv/Scripts/*.exe
        assert_eq!(
            venv_bin_path(&base, "pip", false),
            base.join("pyenv").join("bin").join("pip")
        );
        assert_eq!(
            venv_bin_path(&base, "pylsp", false),
            base.join("pyenv").join("bin").join("pylsp")
        );
        assert_eq!(
            venv_bin_path(&base, "pip", true),
            base.join("pyenv").join("Scripts").join("pip.exe")
        );
        assert_eq!(
            venv_bin_path(&base, "pylsp", true),
            base.join("pyenv").join("Scripts").join("pylsp.exe")
        );
    }

    #[test]
    fn pip_install_args_shape() {
        assert_eq!(
            pip_install_args("python-lsp-server"),
            vec!["install".to_string(), "python-lsp-server".to_string()]
        );
    }

    // ---- resolve active adapter ----

    #[test]
    fn resolve_active_no_workspace_prefers_global_then_adapter_default() {
        let mut cfg = lsp_config::LspConfig::default();
        // No config -> the language's curated default_id.
        assert_eq!(
            resolve_active(&cfg, None, "python").as_deref(),
            Some("pyright")
        );
        assert_eq!(
            resolve_active(&cfg, None, "markdown").as_deref(),
            Some("marksman")
        );
        // Global default wins over the adapter default.
        cfg.defaults.insert("python".into(), "pylsp".into());
        assert_eq!(
            resolve_active(&cfg, None, "python").as_deref(),
            Some("pylsp")
        );
        // Unknown language -> None.
        assert_eq!(resolve_active(&cfg, None, "go"), None);
    }

    #[test]
    fn resolve_active_workspace_override_wins_over_global_default() {
        // W6A-F1: a workspace override (e.g. python=pylsp) must beat the global /
        // adapter default (pyright), else the one-click install targets the wrong
        // server and the chosen profile stays Missing forever.
        let mut cfg = lsp_config::LspConfig::default();
        cfg.defaults.insert("python".into(), "pyright".into());
        cfg.workspaces.insert(
            "/ws/canon".into(),
            std::collections::BTreeMap::from([("python".to_string(), "pylsp".to_string())]),
        );
        assert_eq!(
            resolve_active(&cfg, Some("/ws/canon"), "python").as_deref(),
            Some("pylsp"),
            "workspace override must win"
        );
        // A workspace without an override falls back to the global default.
        assert_eq!(
            resolve_active(&cfg, Some("/ws/other"), "python").as_deref(),
            Some("pyright")
        );
    }

    #[test]
    fn canonical_key_agrees_with_write_side_and_hits_override() {
        // W6A-F1: the set_server write side keys overrides by the *canonical* path;
        // the install read side must canonicalize the raw path the same way (raw and
        // canonical differ on macOS's symlinked tmpdir) or the override is missed.
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().to_str().unwrap();
        let write_key = lsp_config::canonicalize(raw).unwrap_or_else(|| raw.to_string());
        let mut cfg = lsp_config::LspConfig::default();
        cfg.workspaces.insert(
            write_key.clone(),
            std::collections::BTreeMap::from([("python".to_string(), "pylsp".to_string())]),
        );
        let read_key = canonical_key(Some(raw)).expect("some workspace");
        assert_eq!(
            read_key, write_key,
            "read/write canonicalization must agree"
        );
        assert_eq!(
            resolve_active(&cfg, Some(&read_key), "python").as_deref(),
            Some("pylsp")
        );
        // None workspace canonicalizes to None (global-only resolution).
        assert_eq!(canonical_key(None), None);
    }

    // ---- missing-tool branches ----

    #[test]
    fn build_plan_npm_missing_errs_nodejs() {
        let route = route_for("typescript", "vtsls").unwrap();
        let e = build_plan("typescript", route, None, None).unwrap_err();
        assert!(e.contains("Node.js"), "expected a Node.js hint, got: {e}");
    }

    #[test]
    fn build_plan_pip_missing_errs_python() {
        let route = route_for("python", "pylsp").unwrap();
        let e = build_plan("python", route, None, None).unwrap_err();
        assert!(e.contains("Python"), "expected a Python hint, got: {e}");
    }

    #[test]
    fn build_plan_npm_present_binds_resolved_npm() {
        let route = route_for("python", "pyright").unwrap();
        let plan = build_plan("python", route, Some("/usr/bin/npm".into()), None).unwrap();
        assert_eq!(
            plan,
            Plan::Npm {
                npm: "/usr/bin/npm".into(),
                packages: &["pyright"],
                bin: "pyright-langserver"
            }
        );
    }

    #[test]
    fn build_plan_pip_present_binds_resolved_python() {
        let route = route_for("python", "pylsp").unwrap();
        let plan = build_plan("python", route, None, Some("/usr/bin/python3".into())).unwrap();
        assert_eq!(
            plan,
            Plan::Pip {
                python: "/usr/bin/python3".into(),
                package: "python-lsp-server",
                bin: "pylsp"
            }
        );
    }

    #[test]
    fn build_plan_binary_ignores_toolchain() {
        let route = route_for("rust", "rust-analyzer").unwrap();
        assert_eq!(
            build_plan("rust", route, None, None).unwrap(),
            Plan::Binary(BinaryServer::RustAnalyzer)
        );
    }
}
