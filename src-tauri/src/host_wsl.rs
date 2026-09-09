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

#[tauri::command]
pub async fn host_wsl_distributions() -> Result<Vec<WslDistribution>, String> {
    #[cfg(windows)]
    {
        return registrations().await;
    }
    #[cfg(not(windows))]
    Ok(Vec::new())
}

pub(crate) async fn verify_identity(host_id: &str, distro: &str) -> Result<(), String> {
    let entries = host_wsl_distributions().await?;
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
    host_id: String,
    distro: String,
    path: String,
) -> Result<String, String> {
    verify_identity(&host_id, &distro).await?;
    let path = match normalize_windows_folder(&path, &distro)? {
        WslPathInput::Linux(path) => return Ok(path),
        WslPathInput::Windows(path) => path,
    };
    let script = format!("wslpath -a -u {}", crate::host_service::shell_quote(&path)?);
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

#[cfg(test)]
mod tests {
    use super::*;

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
