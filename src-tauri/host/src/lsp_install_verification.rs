pub fn verify_managed_install(
    language: &str,
    server_id: &str,
    installed: &str,
) -> Result<(), String> {
    if crate::lsp_plan::route_for(language, server_id).is_err() {
        return Ok(());
    }
    crate::lsp_catalog::verify_installed(
        crate::lsp_catalog::embedded_catalog()?,
        language,
        server_id,
        std::path::Path::new(installed),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .map(|_| ())
}

/// Remote workspaces may use the account's existing PATH installation. Anything
/// under the Yuzora-owned prefix still requires the reviewed catalog, including
/// a symlink into or out of that prefix. Workspace trust is checked by the host.
pub fn verify_launch(
    language: &str,
    server_id: &str,
    installed: &str,
    host_executables: bool,
) -> Result<(), String> {
    if host_executables {
        let prefix = dirs::home_dir()
            .ok_or("host-home-unavailable")?
            .join(".yuzora/servers");
        if is_host_install(std::path::Path::new(installed), &prefix)? {
            return Ok(());
        }
    }
    verify_managed_install(language, server_id, installed)
}

fn is_host_install(path: &std::path::Path, prefix: &std::path::Path) -> Result<bool, String> {
    let canonical = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let canonical_prefix = std::fs::canonicalize(prefix).unwrap_or_else(|_| prefix.to_owned());
    Ok(!path.starts_with(prefix) && !canonical.starts_with(canonical_prefix))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn host_policy_does_not_exempt_managed_binaries_or_symlink_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("servers");
        std::fs::create_dir(&prefix).unwrap();
        let managed = prefix.join("rust-analyzer");
        let external = temp.path().join("rust-analyzer");
        std::fs::write(&managed, "tampered managed binary").unwrap();
        std::fs::write(&external, "host installation").unwrap();
        assert!(is_host_install(&external, &prefix).unwrap());
        assert!(!is_host_install(&managed, &prefix).unwrap());
        assert!(
            verify_managed_install("rust", "rust-analyzer", managed.to_str().unwrap()).is_err()
        );
        let into = temp.path().join("alias-into-managed");
        let out = prefix.join("alias-to-host");
        symlink(&managed, &into).unwrap();
        symlink(&external, &out).unwrap();
        assert!(!is_host_install(&into, &prefix).unwrap());
        assert!(!is_host_install(&out, &prefix).unwrap());
        let alias_prefix = temp.path().join("servers-alias");
        symlink(&prefix, &alias_prefix).unwrap();
        assert!(!is_host_install(&alias_prefix.join("rust-analyzer"), &prefix).unwrap());
        assert!(!is_host_install(&managed, &alias_prefix).unwrap());
    }
}
