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

#[tauri::command]
pub async fn host_wsl_path(
    ssh: tauri::State<'_, crate::ssh_service::SshState>,
    host_id: String,
    distro: String,
    path: String,
) -> Result<String, String> {
    verify_identity(&host_id, &distro).await?;
    if !((path.len() > 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':')
        || path.starts_with("\\\\"))
    {
        return Err("select-an-absolute-windows-folder".into());
    }
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
