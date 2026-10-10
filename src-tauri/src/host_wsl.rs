//! WSL identity comes from the distribution registration, never its display name.
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WslDistribution {
    pub host_id: String,
    pub name: String,
    pub version: u32,
}

#[cfg(windows)]
async fn registrations() -> Result<Vec<WslDistribution>, String> {
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$result = @(Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' -ErrorAction SilentlyContinue | ForEach-Object {
  $entry = Get-ItemProperty $_.PSPath
  if ($entry.DistributionName) {
    [pscustomobject]@{
      hostId = 'wsl:' + $_.PSChildName + ':' + [string]$entry.DefaultUid
      name = [string]$entry.DistributionName
      version = [int]$entry.Version
    }
  }
})
ConvertTo-Json -InputObject $result -Compress
"#;
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("wsl-probe-stdout-missing")?;
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        stdout
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("wsl-probe-too-large".to_string());
        }
        if !child.wait().await.map_err(|e| e.to_string())?.success() {
            return Err("wsl-discovery-failed".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "wsl-discovery-timeout".to_owned())?
}

async fn list_distributions() -> Result<Vec<WslDistribution>, String> {
    #[cfg(windows)]
    {
        return registrations().await;
    }
    #[cfg(not(windows))]
    Ok(Vec::new())
}

#[tauri::command]
pub async fn host_wsl_distributions(
    preferences: tauri::State<'_, crate::runtime_preferences::RuntimePreferencesState>,
) -> Result<Vec<WslDistribution>, String> {
    crate::runtime_preferences::require_wsl_flag(preferences.wsl_enabled(), true)?;
    list_distributions().await
}

pub(crate) async fn verify_identity(host_id: &str, distro: &str) -> Result<(), String> {
    let entries = list_distributions().await?;
    if !entries
        .iter()
        .any(|entry| entry.host_id == host_id && entry.name == distro && entry.version == 2)
    {
        return Err("wsl-identity-changed-or-not-wsl2".into());
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum WslPathInput {
    Windows(String),
    Linux(String),
}

fn normalize_windows_folder(path: &str, distro: &str) -> Result<WslPathInput, String> {
    if path.contains(['\0', '\r', '\n']) {
        return Err("select-an-absolute-windows-folder".into());
    }
    let mut path = path.replace('/', "\\");
    if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("\\\\?\\UNC\\"))
    {
        path = format!("\\\\{}", &path[8..]);
    } else if path.starts_with("\\\\?\\") {
        path = path[4..].to_string();
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return Ok(WslPathInput::Windows(path));
    }
    if let Some(unc) = path.strip_prefix("\\\\") {
        let parts: Vec<_> = unc.split('\\').collect();
        if parts.len() < 2
            || parts[0].is_empty()
            || parts[1].is_empty()
            || matches!(parts[0], "." | "?")
        {
            return Err("select-an-absolute-windows-folder".into());
        }
        if parts[0].eq_ignore_ascii_case("wsl$") || parts[0].eq_ignore_ascii_case("wsl.localhost") {
            if !parts[1].eq_ignore_ascii_case(distro) {
                return Err("windows-folder-belongs-to-another-wsl-distribution".into());
            }
            return Ok(WslPathInput::Linux(format!("/{}", parts[2..].join("/"))));
        }
        return Ok(WslPathInput::Windows(path));
    }
    Err("select-an-absolute-windows-folder".into())
}

#[tauri::command]
pub async fn host_wsl_path(
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    preferences: tauri::State<'_, crate::runtime_preferences::RuntimePreferencesState>,
    host_id: String,
    distro: String,
    path: String,
) -> Result<String, String> {
    crate::runtime_preferences::require_wsl_flag(preferences.wsl_enabled(), true)?;
    verify_identity(&host_id, &distro).await?;
    let path = match normalize_windows_folder(&path, &distro)? {
        WslPathInput::Linux(path) => return Ok(path),
        WslPathInput::Windows(path) => path,
    };
    let script = format!("wslpath -a -u {}", crate::host_service::shell_quote(&path)?);
    // WSL may have been turned off while the identity check ran.
    crate::runtime_preferences::require_wsl_flag(preferences.wsl_enabled(), true)?;
    let bytes = crate::host_bootstrap::execute(
        &crate::host_service::HostTarget::Wsl { distro },
        Some(&ssh.0),
        &script,
        &[],
    )
    .await?;
    let path = String::from_utf8(bytes).map_err(|_| "wsl-path-not-utf8")?;
    let path = path.strip_suffix('\n').unwrap_or(&path).to_owned();
    if !path.starts_with('/') || path.contains('\0') {
        return Err("wsl-path-conversion-failed".into());
    }
    Ok(path)
}

/// Windows or WSL spelling of each source, ready for conversion. Drive paths
/// are converted by `wslpath`; `\\wsl.localhost\<this distro>` paths map directly.
/// Other shares are refused: `wslpath` cannot reach them.
fn plan_sources(sources: &[std::path::PathBuf], distro: &str) -> Result<Vec<WslPathInput>, String> {
    sources
        .iter()
        .map(|source| {
            let source = source.to_str().ok_or("import-source-invalid")?;
            match normalize_windows_folder(source, distro)? {
                WslPathInput::Windows(path) if path.starts_with("\\\\") => {
                    Err("wsl-import-unsupported-source".into())
                }
                // The bare distro root maps to "/"; the helper would reject it with a raw code.
                WslPathInput::Linux(path) if path.trim_matches('/').is_empty() => {
                    Err("wsl-import-unsupported-source".into())
                }
                planned => Ok(planned),
            }
        })
        .collect()
}

/// Authorise the requested paths against the recorded native drops BEFORE any
/// of them is interpreted or converted.
fn authorise_and_plan(
    recent: &crate::file_clipboard::RecentDrop,
    requested: &[String],
    distro: &str,
    now: std::time::Instant,
) -> Result<Vec<WslPathInput>, String> {
    plan_sources(&recent.take(requested, now)?, distro)
}

/// One `wslpath` call per drive path; None when nothing needs converting.
fn conversion_script(planned: &[WslPathInput]) -> Result<Option<String>, String> {
    let mut script = String::new();
    for input in planned {
        if let WslPathInput::Windows(path) = input {
            script.push_str(&format!(
                "wslpath -a -u {}\n",
                crate::host_service::shell_quote(path)?
            ));
        }
    }
    Ok((!script.is_empty()).then_some(script))
}

/// Linux paths in request order: direct ones kept, converted ones taken from
/// `output` (one line per drive path). Every result must be an absolute path.
fn merge_converted(planned: Vec<WslPathInput>, output: &[u8]) -> Result<Vec<String>, String> {
    let text = std::str::from_utf8(output).map_err(|_| "wsl-path-not-utf8")?;
    let mut converted = text.strip_suffix('\n').unwrap_or(text).split('\n');
    planned
        .into_iter()
        .map(|input| {
            let path = match input {
                WslPathInput::Linux(path) => path,
                WslPathInput::Windows(_) => converted
                    .next()
                    .ok_or("wsl-path-conversion-failed")?
                    .to_owned(),
            };
            if !path.starts_with('/') || path.contains('\0') {
                return Err("wsl-path-conversion-failed".to_string());
            }
            Ok(path)
        })
        .collect()
}

/// The WSL workspace folder an import lands in.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslImportDestination {
    owner: yuzora_host::protocol::ConnectionOwner,
    distro: String,
    /// Workspace capability id on the host.
    workspace: String,
    /// Workspace-relative folder.
    target_dir: String,
}

/// Cheap synchronous preconditions of an import. Run BEFORE a native drop is
/// consumed, so a failure here leaves the drop available for a retry.
fn check_import_preconditions(
    state: &crate::host_service::HostState,
    preferences: &crate::runtime_preferences::RuntimePreferencesState,
    destination: &WslImportDestination,
) -> Result<(), String> {
    use crate::host_service::HostTarget;
    crate::runtime_preferences::require_wsl_flag(preferences.wsl_enabled(), true)?;
    let connection = state.0.connection(&destination.owner)?;
    if !matches!(&connection.target, HostTarget::Wsl { distro: d } if *d == destination.distro) {
        return Err("host-not-wsl-distribution".into());
    }
    if connection.files_import.get() != Some(&true) {
        return Err("wsl-helper-outdated".into());
    }
    Ok(())
}

/// `authorise_and_plan` that runs `preconditions` first and leaves the drop untouched when they fail.
fn authorise_after_preconditions(
    preconditions: impl FnOnce() -> Result<(), String>,
    recent: &crate::file_clipboard::RecentDrop,
    requested: &[String],
    distro: &str,
    now: std::time::Instant,
) -> Result<Vec<WslPathInput>, String> {
    preconditions()?;
    authorise_and_plan(recent, requested, distro, now)
}

/// Copy `planned` sources into a WSL workspace through the host helper. The
/// sources must already be authorised by the caller (native drop or clipboard).
async fn import_into_wsl(
    state: &crate::host_service::HostState,
    ssh: &crate::ssh_service::SshState,
    preferences: &crate::runtime_preferences::RuntimePreferencesState,
    destination: WslImportDestination,
    planned: Vec<WslPathInput>,
) -> Result<Vec<String>, String> {
    use crate::host_service::HostTarget;
    check_import_preconditions(state, preferences, &destination)?;
    let WslImportDestination {
        owner,
        distro,
        workspace,
        target_dir,
    } = destination;
    verify_identity(&owner.host_id, &distro).await?;
    let target = HostTarget::Wsl {
        distro: distro.clone(),
    };
    let converted = match conversion_script(&planned)? {
        Some(script) => {
            // WSL may have been turned off while the identity check ran.
            crate::runtime_preferences::require_wsl_flag(preferences.wsl_enabled(), true)?;
            crate::host_bootstrap::execute(&target, Some(&ssh.0), &script, &[]).await?
        }
        None => Vec::new(),
    };
    let sources = merge_converted(planned, &converted)?;
    crate::runtime_preferences::require_wsl_flag(preferences.wsl_enabled(), true)?;
    let connection = state.0.connection(&owner)?;
    let created = connection
        .request_with_timeout(
            yuzora_host::protocol::Operation::FilesImport {
                workspace,
                sources,
                target_dir,
            },
            std::time::Duration::from_secs(130),
        )
        .await?;
    state.0.connection(&owner)?;
    serde_json::from_value(created).map_err(|e| e.to_string())
}

/// Finder / Explorer drop into a WSL workspace. `paths` must come from the
/// latest native drop; the renderer never names a host path on its own.
#[tauri::command]
pub async fn host_import_dropped_paths(
    state: tauri::State<'_, crate::host_service::HostState>,
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    preferences: tauri::State<'_, crate::runtime_preferences::RuntimePreferencesState>,
    destination: WslImportDestination,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let planned = crate::file_clipboard::retry_until_recorded(|| {
        authorise_after_preconditions(
            || check_import_preconditions(&state, &preferences, &destination),
            &crate::file_clipboard::RECENT_DROP,
            &paths,
            &destination.distro,
            std::time::Instant::now(),
        )
    })
    .await?;
    import_into_wsl(&state, &ssh, &preferences, destination, planned).await
}

/// Paste the OS clipboard's file list (read natively, never from JS) into a WSL workspace.
#[tauri::command]
pub async fn host_paste_clipboard_files(
    state: tauri::State<'_, crate::host_service::HostState>,
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    preferences: tauri::State<'_, crate::runtime_preferences::RuntimePreferencesState>,
    destination: WslImportDestination,
) -> Result<Vec<String>, String> {
    let clipboard =
        crate::fs_service::run_blocking(crate::file_clipboard::os_read_file_list).await?;
    if clipboard.is_empty() {
        return Ok(Vec::new());
    }
    let planned = plan_sources(&clipboard, &destination.distro)?;
    import_into_wsl(&state, &ssh, &preferences, destination, planned).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn plans_drive_paths_for_conversion_and_same_distro_unc_paths_directly() {
        let planned = plan_sources(
            &paths(&[
                r"C:\Users\me\a b.txt",
                r"\\wsl.localhost\Ubuntu\home\me\x",
                r"\\wsl$\Ubuntu\y",
                r"\\?\D:\data",
            ]),
            "Ubuntu",
        )
        .unwrap();
        assert_eq!(
            planned,
            vec![
                WslPathInput::Windows(r"C:\Users\me\a b.txt".into()),
                WslPathInput::Linux("/home/me/x".into()),
                WslPathInput::Linux("/y".into()),
                WslPathInput::Windows(r"D:\data".into()),
            ]
        );
    }

    #[test]
    fn rejects_other_distributions_other_shares_and_relative_sources() {
        assert_eq!(
            plan_sources(&paths(&[r"\\wsl.localhost\Debian\home\x"]), "Ubuntu").unwrap_err(),
            "windows-folder-belongs-to-another-wsl-distribution"
        );
        assert_eq!(
            plan_sources(&paths(&[r"\\server\share\x"]), "Ubuntu").unwrap_err(),
            "wsl-import-unsupported-source"
        );
        for bad in ["relative/a", "/mnt/c/a", r"C:relative"] {
            assert!(plan_sources(&paths(&[bad]), "Ubuntu").is_err(), "{bad}");
        }
    }

    #[test]
    fn builds_one_quoted_wslpath_call_per_drive_path_only() {
        let planned = vec![
            WslPathInput::Windows(r"C:\it's a dir\f.txt".into()),
            WslPathInput::Linux("/home/me/x".into()),
            WslPathInput::Windows(r"D:\b".into()),
        ];
        assert_eq!(
            conversion_script(&planned).unwrap().unwrap(),
            "wslpath -a -u 'C:\\it'\\''s a dir\\f.txt'\nwslpath -a -u 'D:\\b'\n"
        );
        assert_eq!(
            conversion_script(&[WslPathInput::Linux("/x".into())]).unwrap(),
            None
        );
    }

    #[test]
    fn merges_converted_paths_in_request_order_and_requires_absolute_results() {
        let planned = || {
            vec![
                WslPathInput::Windows(r"C:\a".into()),
                WslPathInput::Linux("/home/me/x".into()),
                WslPathInput::Windows(r"D:\b".into()),
            ]
        };
        assert_eq!(
            merge_converted(planned(), b"/mnt/c/a\n/mnt/d/b\n").unwrap(),
            vec!["/mnt/c/a", "/home/me/x", "/mnt/d/b"]
        );
        for bad in [&b"/mnt/c/a\n"[..], b"/mnt/c/a\nrelative\n", b"\n\n", b""] {
            assert_eq!(
                merge_converted(planned(), bad).unwrap_err(),
                "wsl-path-conversion-failed"
            );
        }
        assert_eq!(
            merge_converted(planned(), b"\xff\xfe").unwrap_err(),
            "wsl-path-not-utf8"
        );
    }

    #[test]
    fn authorises_before_interpreting_any_path() {
        let now = Instant::now();
        let recent = crate::file_clipboard::RecentDrop::for_test(
            vec![paths(&[r"\\wsl.localhost\Debian\secret", r"C:\ok\a.txt"])],
            now,
        );
        // A path that was never dropped is "unknown" even when it would also be
        // rejected by the mapping (other distribution, other share, relative).
        for requested in [
            r"\\wsl.localhost\Debian\other",
            r"\\server\share\x",
            "relative",
            r"C:\not\dropped.txt",
        ] {
            assert_eq!(
                authorise_and_plan(&recent, &[requested.to_string()], "Ubuntu", now).unwrap_err(),
                "dropped-paths-unknown",
                "{requested}"
            );
        }
        // An authorised path from another distribution is then refused by the mapping.
        assert_eq!(
            authorise_and_plan(
                &recent,
                &[r"\\wsl.localhost\Debian\secret".to_string()],
                "Ubuntu",
                now
            )
            .unwrap_err(),
            "windows-folder-belongs-to-another-wsl-distribution"
        );
    }

    #[test]
    fn a_drop_is_consumed_by_the_wsl_import_authorisation() {
        let now = Instant::now();
        let recent =
            crate::file_clipboard::RecentDrop::for_test(vec![paths(&[r"C:\ok\a.txt"])], now);
        let requested = [r"C:\ok\a.txt".to_string()];
        assert_eq!(
            authorise_and_plan(&recent, &requested, "Ubuntu", now).unwrap(),
            vec![WslPathInput::Windows(r"C:\ok\a.txt".into())]
        );
        assert_eq!(
            authorise_and_plan(&recent, &requested, "Ubuntu", now).unwrap_err(),
            "dropped-paths-unknown"
        );
    }

    #[test]
    fn a_failed_precondition_leaves_the_drop_available() {
        let now = Instant::now();
        let recent =
            crate::file_clipboard::RecentDrop::for_test(vec![paths(&[r"C:\ok\a.txt"])], now);
        let requested = [r"C:\ok\a.txt".to_string()];
        assert_eq!(
            authorise_after_preconditions(
                || Err("wsl-helper-outdated".into()),
                &recent,
                &requested,
                "Ubuntu",
                now
            )
            .unwrap_err(),
            "wsl-helper-outdated"
        );
        assert_eq!(
            authorise_after_preconditions(|| Ok(()), &recent, &requested, "Ubuntu", now).unwrap(),
            vec![WslPathInput::Windows(r"C:\ok\a.txt".into())]
        );
    }

    #[test]
    fn rejects_the_bare_distro_root_as_an_import_source() {
        for root in [
            r"\\wsl.localhost\Ubuntu",
            r"\\wsl$\Ubuntu\\",
            r"\\wsl.localhost\Ubuntu\\",
        ] {
            assert_eq!(
                plan_sources(&paths(&[root]), "Ubuntu").unwrap_err(),
                "wsl-import-unsupported-source",
                "{root}"
            );
        }
    }

    #[test]
    fn wsl_opt_out_is_refused_by_the_flag_check_used_around_each_await() {
        assert_eq!(
            crate::runtime_preferences::require_wsl_flag(false, true).unwrap_err(),
            crate::runtime_preferences::WSL_DISABLED_ERROR
        );
        assert!(crate::runtime_preferences::require_wsl_flag(true, true).is_ok());
    }

    #[test]
    fn normalizes_absolute_drive_and_verbatim_paths_without_losing_unicode() {
        for path in [
            r"C:\專案 files\app",
            r"\\?\C:\專案 files\app",
            "C:/專案 files/app",
        ] {
            assert_eq!(
                normalize_windows_folder(path, "Ubuntu").unwrap(),
                WslPathInput::Windows(r"C:\專案 files\app".into())
            );
        }
        assert_eq!(
            normalize_windows_folder(r"\\?\UNC\server\share\project", "Ubuntu").unwrap(),
            WslPathInput::Windows(r"\\server\share\project".into())
        );
    }

    #[test]
    fn wsl_unc_paths_are_bound_to_the_selected_distribution() {
        for path in [
            r"\\wsl$\Ubuntu\home\中文 project",
            r"\\wsl.localhost\Ubuntu\home\中文 project",
            r"\\?\UNC\wsl.localhost\Ubuntu\home\中文 project",
        ] {
            assert_eq!(
                normalize_windows_folder(path, "Ubuntu").unwrap(),
                WslPathInput::Linux("/home/中文 project".into())
            );
            assert_eq!(
                normalize_windows_folder(path, "Debian").unwrap_err(),
                "windows-folder-belongs-to-another-wsl-distribution"
            );
        }
        assert_eq!(
            normalize_windows_folder(r"\\wsl$\Ubuntu", "Ubuntu").unwrap(),
            WslPathInput::Linux("/".into())
        );
    }

    #[test]
    fn rejects_drive_relative_device_and_incomplete_unc_paths() {
        for path in [
            "C:relative",
            "C:",
            "relative",
            r"\\server",
            r"\\server\",
            r"\\.\pipe\name",
            "C:\\bad\npath",
            "C:\\bad\0path",
        ] {
            assert!(
                normalize_windows_folder(path, "Ubuntu").is_err(),
                "{path:?}"
            );
        }
    }
}
