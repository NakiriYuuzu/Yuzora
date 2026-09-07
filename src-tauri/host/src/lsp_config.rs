use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LspConfig {
    pub defaults: BTreeMap<String, String>,
    pub workspaces: BTreeMap<String, BTreeMap<String, String>>,
}

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".yuzora")
        .join("lsp.json")
}

pub fn load_from(path: &Path) -> LspConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_to(path: &Path, cfg: &LspConfig) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize failed: {e}"))?;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("lsp-config-parent-missing")?)
            .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Separate helpers for one account may edit defaults concurrently. Serialize
/// read/modify/replace across processes and retain malformed configs for repair.
pub fn update(
    path: &Path,
    change: impl FnOnce(&mut LspConfig) -> Result<(), String>,
) -> Result<LspConfig, String> {
    let parent = path.parent().ok_or("lsp-config-parent-missing")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options
        .open(path.with_extension("lock"))
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(error) => return Err(format!("lsp-config-lock: {error}")),
        }
    }
    let mut cfg = match std::fs::File::open(path) {
        Ok(file) => {
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("lsp-config-too-large".into());
            }
            serde_json::from_slice(&bytes).map_err(|e| format!("lsp-config-invalid: {e}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LspConfig::default(),
        Err(error) => return Err(error.to_string()),
    };
    change(&mut cfg)?;
    save_to(path, &cfg)?;
    Ok(cfg)
}

pub fn canonicalize(path: &str) -> Option<String> {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn resolve_server(cfg: &LspConfig, ws_canonical: &str, language: &str) -> Option<String> {
    cfg.workspaces
        .get(ws_canonical)
        .and_then(|m| m.get(language))
        .or_else(|| cfg.defaults.get(language))
        .cloned()
}

pub fn set_server(cfg: &mut LspConfig, ws_canonical: Option<&str>, language: &str, id: &str) {
    match ws_canonical {
        None => {
            cfg.defaults.insert(language.to_string(), id.to_string());
        }
        Some(ws) => {
            cfg.workspaces
                .entry(ws.to_string())
                .or_default()
                .insert(language.to_string(), id.to_string());
        }
    }
}

pub fn stale_workspaces(cfg: &LspConfig) -> Vec<String> {
    cfg.workspaces
        .keys()
        .filter(|ws| !Path::new(ws).exists())
        .cloned()
        .collect()
}

pub fn clear_workspace(cfg: &mut LspConfig, ws: &str) {
    cfg.workspaces.remove(ws);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_updates_preserve_other_workspace_settings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lsp.json");
        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    update(path, |cfg| {
                        set_server(
                            cfg,
                            Some(&format!("/workspace-{index}")),
                            "rust",
                            "rust-analyzer",
                        );
                        Ok(())
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(load_from(&path).workspaces.len(), 8);
    }

    #[test]
    fn malformed_config_is_preserved_instead_of_overwritten_by_an_update() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lsp.json");
        std::fs::write(&path, "{damaged").unwrap();
        assert!(update(&path, |_| Ok(())).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{damaged");
    }

    #[test]
    fn config_path_ends_with_yuzora_lsp_json() {
        let p = config_path();
        assert!(p.ends_with(".yuzora/lsp.json"), "got {p:?}");
    }

    #[test]
    fn load_from_missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.json");
        assert_eq!(load_from(&path), LspConfig::default());
    }

    #[test]
    fn load_from_bad_json_returns_default_no_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lsp.json");
        std::fs::write(&path, "{ not valid json ][").unwrap();
        assert_eq!(load_from(&path), LspConfig::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        // Nested dir that does not exist yet — save_to must create it.
        let path = tmp.path().join("nested").join("lsp.json");
        let mut cfg = LspConfig::default();
        cfg.defaults.insert("python".into(), "pyright".into());
        cfg.workspaces.insert(
            "/ws/a".into(),
            BTreeMap::from([("rust".to_string(), "rust-analyzer".to_string())]),
        );
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path), cfg);
    }

    #[test]
    fn save_to_writes_pretty_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lsp.json");
        let mut cfg = LspConfig::default();
        cfg.defaults.insert("python".into(), "pyright".into());
        save_to(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains('\n'), "pretty JSON should be multi-line");
    }

    #[test]
    fn canonicalize_existing_path_returns_some() {
        let tmp = tempfile::tempdir().unwrap();
        let got = canonicalize(tmp.path().to_str().unwrap());
        assert!(got.is_some());
    }

    #[test]
    fn canonicalize_missing_path_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert_eq!(canonicalize(missing.to_str().unwrap()), None);
    }

    #[test]
    fn resolve_server_workspace_override_wins_over_defaults() {
        let mut cfg = LspConfig::default();
        cfg.defaults.insert("python".into(), "pyright".into());
        cfg.workspaces.insert(
            "/ws/a".into(),
            BTreeMap::from([("python".to_string(), "pylsp".to_string())]),
        );
        assert_eq!(
            resolve_server(&cfg, "/ws/a", "python"),
            Some("pylsp".into())
        );
    }

    #[test]
    fn resolve_server_falls_back_to_defaults() {
        let mut cfg = LspConfig::default();
        cfg.defaults.insert("python".into(), "pyright".into());
        assert_eq!(
            resolve_server(&cfg, "/ws/other", "python"),
            Some("pyright".into())
        );
    }

    #[test]
    fn resolve_server_no_match_returns_none() {
        let cfg = LspConfig::default();
        assert_eq!(resolve_server(&cfg, "/ws/a", "python"), None);
    }

    #[test]
    fn set_server_none_writes_defaults() {
        let mut cfg = LspConfig::default();
        set_server(&mut cfg, None, "rust", "rust-analyzer");
        assert_eq!(cfg.defaults.get("rust"), Some(&"rust-analyzer".to_string()));
        assert!(cfg.workspaces.is_empty());
    }

    #[test]
    fn set_server_some_writes_workspace() {
        let mut cfg = LspConfig::default();
        set_server(&mut cfg, Some("/ws/a"), "rust", "rust-analyzer");
        assert_eq!(
            cfg.workspaces.get("/ws/a").and_then(|m| m.get("rust")),
            Some(&"rust-analyzer".to_string())
        );
        assert!(cfg.defaults.is_empty());
    }

    #[test]
    fn clear_workspace_removes_key() {
        let mut cfg = LspConfig::default();
        set_server(&mut cfg, Some("/ws/a"), "rust", "rust-analyzer");
        clear_workspace(&mut cfg, "/ws/a");
        assert!(cfg.workspaces.is_empty());
    }

    #[test]
    fn stale_workspaces_reports_missing_paths_only() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().to_str().unwrap().to_string();
        let missing = tmp.path().join("gone").to_str().unwrap().to_string();
        let mut cfg = LspConfig::default();
        cfg.workspaces.insert(existing.clone(), BTreeMap::new());
        cfg.workspaces.insert(missing.clone(), BTreeMap::new());
        let stale = stale_workspaces(&cfg);
        assert_eq!(stale, vec![missing]);
    }
}
