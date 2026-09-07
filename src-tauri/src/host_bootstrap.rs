//! Guided installation into a content-addressed directory owned by Yuzora.
use crate::host_service::{
    shell_quote, spawn_process_stream, ConnectedHost, HostState, HostTarget,
};
use crate::ssh_service::{SshManager, SshState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::Manager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yuzora_host::protocol::{Operation, PROTOCOL_VERSION};

const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const SUCCESS: &[u8] = b"\0YUZORA_OK\0";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProbe {
    pub os: String,
    pub arch: String,
    pub home: String,
    pub installed_herdr: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactFile {
    path: String,
    sha256: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HerdrArtifact {
    path: String,
    sha256: String,
    version: String,
    protocol: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol: u32,
    version: String,
    target: String,
    helper: ArtifactFile,
    herdr: HerdrArtifact,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedHost {
    pub connection: ConnectedHost,
    pub binary: String,
    pub helper: String,
}

pub(crate) async fn execute(
    target: &HostTarget,
    ssh: Option<&SshManager>,
    script: &str,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let script = format!("set -eu\n{script}\nprintf '\\000YUZORA_OK\\000'\n");
    let mut stream = match target {
        HostTarget::Ssh { session_id } => {
            ssh.ok_or("ssh-connection-required")?
                .open_host_exec(session_id, &format!("sh -c {}", shell_quote(&script)?))
                .await?
        }
        HostTarget::Wsl { distro } => {
            if !cfg!(windows) {
                return Err("wsl-requires-windows".into());
            }
            if distro.is_empty() || distro.contains('\0') {
                return Err("invalid-wsl-distro".into());
            }
            spawn_process_stream(tokio::process::Command::new("wsl.exe").args([
                "--distribution",
                distro,
                "--exec",
                "sh",
                "-c",
                &script,
            ]))?
        }
        HostTarget::Local => {
            if cfg!(windows) {
                return Err("select-wsl-runtime".into());
            }
            spawn_process_stream(tokio::process::Command::new("sh").args(["-c", &script]))?
        }
    };
    tokio::time::timeout(Duration::from_secs(60), async {
        stream.write_all(input).await.map_err(|e| e.to_string())?;
        stream.shutdown().await.map_err(|e| e.to_string())?;
        let mut output = Vec::new();
        (&mut stream)
            .take(65537)
            .read_to_end(&mut output)
            .await
            .map_err(|e| e.to_string())?;
        if output.len() > 65536 {
            return Err("host-probe-output-too-large".into());
        }
        if !output.ends_with(SUCCESS) {
            return Err("host-setup-command-failed".into());
        }
        output.truncate(output.len() - SUCCESS.len());
        Ok(output)
    })
    .await
    .map_err(|_| "host-setup-timeout".to_owned())?
}

async fn probe(target: &HostTarget, ssh: &SshManager) -> Result<HostProbe, String> {
    let bytes = execute(target,Some(ssh),r#"printf '%s\000' "$(uname -s)" "$(uname -m)" "$(cd "$HOME" && pwd -P)" "$(command -v herdr || true)""#,&[]).await?;
    let value = String::from_utf8(bytes).map_err(|_| "host-probe-not-utf8")?;
    let fields: Vec<_> = value.split('\0').collect();
    if fields.len() != 5 || !fields[4].is_empty() {
        return Err("invalid-host-probe".into());
    }
    let os = match fields[0] {
        "Linux" => "linux",
        "Darwin" => "macos",
        _ => return Err("unsupported-runtime-os".into()),
    };
    let arch = match fields[1] {
        "aarch64" | "arm64" => "aarch64",
        "x86_64" => "x86_64",
        _ => return Err("unsupported-runtime-architecture".into()),
    };
    if !fields[2].starts_with('/') {
        return Err("host-home-unavailable".into());
    }
    Ok(HostProbe {
        os: os.into(),
        arch: arch.into(),
        home: fields[2].into(),
        installed_herdr: (!fields[3].is_empty()).then(|| fields[3].into()),
    })
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_manifest(manifest: &Manifest, target: &str) -> Result<(), String> {
    if manifest.protocol != PROTOCOL_VERSION
        || manifest.version != env!("CARGO_PKG_VERSION")
        || manifest.target != target
    {
        return Err("host-artifact-version-mismatch".into());
    }
    if manifest.helper.path != format!("{target}/yuzora-host")
        || manifest.herdr.path != format!("{target}/herdr")
    {
        return Err("invalid-host-artifact-path".into());
    }
    if manifest.herdr.version != "0.8.2" || manifest.herdr.protocol != 20 {
        return Err("herdr-artifact-version-mismatch".into());
    }
    for hash in [&manifest.helper.sha256, &manifest.herdr.sha256] {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid-artifact-hash".into());
        }
    }
    Ok(())
}

async fn verified_file(root: &Path, path: &str, expected: &str) -> Result<Vec<u8>, String> {
    let path = root.join(path);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|e| format!("host-artifact-missing: {e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err("invalid-host-artifact".into());
    }
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?
        .take(MAX_ARTIFACT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ARTIFACT_BYTES || hash(&bytes) != expected {
        return Err("host-artifact-hash-mismatch".into());
    }
    Ok(bytes)
}

async fn deploy_file(
    target: &HostTarget,
    ssh: Option<&SshManager>,
    home: &str,
    version_dir: &str,
    name: &str,
    bytes: &[u8],
    digest: &str,
) -> Result<String, String> {
    let root = format!("{home}/.local/share/yuzora/runtimes");
    let directory = format!("{root}/{version_dir}");
    let destination = format!("{directory}/{name}");
    let scratch = format!("{directory}/.upload-{}", uuid::Uuid::new_v4());
    let mut script = String::from("umask 077\n");
    // Never follow a pre-existing symlink while installing managed files.
    for path in [
        format!("{home}/.local"),
        format!("{home}/.local/share"),
        format!("{home}/.local/share/yuzora"),
        root,
        directory,
    ] {
        let path = shell_quote(&path)?;
        script.push_str(&format!(
            "test ! -L {path}\nif test ! -e {path}; then mkdir -- {path}; fi\ntest -d {path}\n"
        ));
    }
    script.push_str(&format!(
        r#"
destination={destination}
scratch={scratch}
expected={digest}
checksum() {{
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -- "$1"; else shasum -a 256 -- "$1"; fi
}}
test ! -L "$destination"
if test -e "$destination"; then
  test -f "$destination"
  actual=$(checksum "$destination")
  test "${{actual%% *}}" = "$expected"
  cat >/dev/null
else
  trap 'rm -f -- "$scratch"' EXIT HUP INT TERM
  (set -C; cat > "$scratch")
  actual=$(checksum "$scratch")
  test "${{actual%% *}}" = "$expected"
  chmod 700 "$scratch"
  # Hard-link promotion refuses concurrent replacement of this immutable file.
  ln "$scratch" "$destination"
  rm -- "$scratch"
  trap - EXIT HUP INT TERM
fi
"#,
        destination = shell_quote(&destination)?,
        scratch = shell_quote(&scratch)?,
        digest = shell_quote(digest)?
    ));
    execute(target, ssh, &script, bytes).await?;
    Ok(destination)
}

fn resource_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let bundled = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("host");
    if bundled.is_dir() {
        return Ok(bundled);
    }
    #[cfg(debug_assertions)]
    {
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/host"))
    }
    #[cfg(not(debug_assertions))]
    Err("host-artifacts-not-bundled".into())
}

#[tauri::command]
pub async fn host_probe(
    ssh: tauri::State<'_, SshState>,
    target: HostTarget,
) -> Result<HostProbe, String> {
    probe(&target, &ssh.0).await
}

#[tauri::command]
pub async fn host_prepare(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostState>,
    ssh: tauri::State<'_, SshState>,
    host_id: String,
    target: HostTarget,
    use_managed_herdr: bool,
) -> Result<PreparedHost, String> {
    if let HostTarget::Wsl { distro } = &target {
        crate::host_wsl::verify_identity(&host_id, distro).await?;
    }
    let info = probe(&target, &ssh.0).await?;
    let platform = format!("{}-{}", info.os, info.arch);
    let root = resource_root(&app)?;
    let manifest_bytes = tokio::fs::read(root.join(format!("{platform}.json")))
        .await
        .map_err(|_| format!("host-artifact-unavailable: {platform}"))?;
    if manifest_bytes.len() > 16384 {
        return Err("host-manifest-too-large".into());
    }
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    validate_manifest(&manifest, &platform)?;
    let helper_bytes = verified_file(&root, &manifest.helper.path, &manifest.helper.sha256).await?;
    let herdr_bytes = verified_file(&root, &manifest.herdr.path, &manifest.herdr.sha256).await?;
    let directory = format!(
        "{}-{}-{}",
        manifest.version,
        platform,
        hash(&manifest_bytes)
    );
    let helper = deploy_file(
        &target,
        Some(&ssh.0),
        &info.home,
        &directory,
        "yuzora-host",
        &helper_bytes,
        &manifest.helper.sha256,
    )
    .await?;
    let binary = if let Some(binary) = info.installed_herdr.filter(|_| !use_managed_herdr) {
        binary
    } else {
        deploy_file(
            &target,
            Some(&ssh.0),
            &info.home,
            &directory,
            "herdr",
            &herdr_bytes,
            &manifest.herdr.sha256,
        )
        .await?
    };
    let connection = state
        .0
        .connect(host_id, target, helper.clone(), &ssh.0)
        .await?;
    if let Err(error) = state
        .0
        .request(
            connection.owner.clone(),
            Operation::HerdrStart {
                binary: binary.clone(),
            },
        )
        .await
    {
        let _ = state.0.disconnect(&connection.owner).await;
        return Err(error);
    }
    Ok(PreparedHost {
        connection,
        binary,
        helper,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn deployment_is_verified_idempotent_and_does_not_replace_existing_bytes() {
        let home = tempfile::tempdir().unwrap();
        let bytes = b"#!/bin/sh\nexit 0\n";
        let path = deploy_file(
            &HostTarget::Local,
            None,
            home.path().to_str().unwrap(),
            "test-version",
            "yuzora-host",
            bytes,
            &hash(bytes),
        )
        .await
        .unwrap();
        deploy_file(
            &HostTarget::Local,
            None,
            home.path().to_str().unwrap(),
            "test-version",
            "yuzora-host",
            bytes,
            &hash(bytes),
        )
        .await
        .unwrap();
        std::fs::write(&path, b"user modification").unwrap();
        assert!(deploy_file(
            &HostTarget::Local,
            None,
            home.path().to_str().unwrap(),
            "test-version",
            "yuzora-host",
            bytes,
            &hash(bytes)
        )
        .await
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"user modification");
    }
}
