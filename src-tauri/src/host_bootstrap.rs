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
use yuzora_host::herdr_limits::MAX_NDJSON_LINE_BYTES;
use yuzora_host::herdr_runtime::{inspect_documents, session_names, RuntimeBinaryCheck};
use yuzora_host::herdr_service::HerdrBinarySource;
use yuzora_host::protocol::{Operation, PROTOCOL_VERSION};

const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PROBE_OUTPUT_BYTES: usize = 64 * 1024;
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
    pub artifact_identity: String,
}

pub(crate) async fn execute(
    target: &HostTarget,
    ssh: Option<&SshManager>,
    script: &str,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    execute_with_output_limit(target, ssh, script, input, MAX_PROBE_OUTPUT_BYTES).await
}

async fn execute_with_output_limit(
    target: &HostTarget,
    ssh: Option<&SshManager>,
    script: &str,
    input: &[u8],
    max_output_bytes: usize,
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
            .take((max_output_bytes + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|e| e.to_string())?;
        if output.len() > max_output_bytes {
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
    let release: serde_json::Value = serde_json::from_str(include_str!("../herdr-runtime.json"))
        .expect("bundled HERDR release manifest must be valid JSON");
    if release["baseVersion"].as_str() != Some(manifest.herdr.version.as_str())
        || release["protocol"].as_u64() != Some(u64::from(manifest.herdr.protocol))
    {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRuntimeCheck {
    pub binary: String,
    pub installed_binary: Option<String>,
    pub managed_version: String,
    pub managed_protocol: u32,
    pub artifact_identity: String,
    pub requires_install: bool,
    pub check: Option<RuntimeBinaryCheck>,
}

fn select_host_binary(
    info: &HostProbe,
    source: HerdrBinarySource,
    custom_path: Option<&str>,
    directory: &str,
) -> Result<String, String> {
    let binary = match source {
        HerdrBinarySource::Default => format!(
            "{}/.local/share/yuzora/runtimes/{directory}/herdr",
            info.home
        ),
        HerdrBinarySource::Global => info
            .installed_herdr
            .clone()
            .ok_or("herdr-not-found-on-selected-host")?,
        HerdrBinarySource::Custom => custom_path.ok_or("herdr-custom-path-required")?.to_string(),
    };
    if !binary.starts_with('/') || binary.contains('\0') {
        return Err("herdr-host-path-must-be-absolute".into());
    }
    Ok(binary)
}

async fn runtime_metadata(
    target: &HostTarget,
    ssh: Option<&SshManager>,
    binary: &str,
    session: &str,
    command: &str,
) -> Result<serde_json::Value, String> {
    let script = format!(
        "HERDR_SESSION={} {} {command}",
        shell_quote(session)?,
        shell_quote(binary)?
    );
    // Runtime schemas exceed the small host-probe budget. Match the native
    // JSON ceiling while allowing the bootstrap transport's success marker.
    let bytes = execute_with_output_limit(
        target,
        ssh,
        &script,
        &[],
        MAX_NDJSON_LINE_BYTES + SUCCESS.len(),
    )
    .await?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid-runtime-json: {error}"))
}

async fn inspect_host_binary(
    target: &HostTarget,
    ssh: &SshManager,
    binary: &str,
) -> Result<RuntimeBinaryCheck, String> {
    let schema =
        runtime_metadata(target, Some(ssh), binary, "default", "api schema --json").await?;
    let sessions =
        runtime_metadata(target, Some(ssh), binary, "default", "session list --json").await?;
    let mut statuses = Vec::new();
    for name in session_names(&sessions)? {
        let status = runtime_metadata(target, Some(ssh), binary, &name, "status --json").await?;
        statuses.push((name, status));
    }
    inspect_documents(binary.to_string(), schema, statuses)
}

#[tauri::command]
pub async fn host_runtime_check(
    app: tauri::AppHandle,
    ssh: tauri::State<'_, SshState>,
    host_id: String,
    target: HostTarget,
    source: HerdrBinarySource,
    custom_path: Option<String>,
) -> Result<HostRuntimeCheck, String> {
    if let HostTarget::Wsl { distro } = &target {
        crate::host_wsl::verify_identity(&host_id, distro).await?;
    }
    let info = probe(&target, &ssh.0).await?;
    let platform = format!("{}-{}", info.os, info.arch);
    let bytes = tokio::fs::read(resource_root(&app)?.join(format!("{platform}.json")))
        .await
        .map_err(|_| format!("host-artifact-unavailable: {platform}"))?;
    if bytes.len() > 16384 {
        return Err("host-manifest-too-large".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate_manifest(&manifest, &platform)?;
    let artifact_identity = hash(&bytes);
    let directory = format!("{}-{platform}-{artifact_identity}", manifest.version);
    let binary = select_host_binary(&info, source, custom_path.as_deref(), &directory)?;
    let exists = execute(
        &target,
        Some(&ssh.0),
        &format!(
            "if test -x {}; then printf yes; else printf no; fi",
            shell_quote(&binary)?
        ),
        &[],
    )
    .await?
        == b"yes";
    if !exists && source != HerdrBinarySource::Default {
        return Err(format!("herdr-not-executable-on-selected-host: {binary}"));
    }
    let check = if exists {
        Some(inspect_host_binary(&target, &ssh.0, &binary).await?)
    } else {
        None
    };
    Ok(HostRuntimeCheck {
        binary,
        installed_binary: info.installed_herdr,
        managed_version: manifest.herdr.version,
        managed_protocol: manifest.herdr.protocol,
        artifact_identity,
        requires_install: !exists,
        check,
    })
}

#[tauri::command]
pub async fn host_prepare(
    app: tauri::AppHandle,
    state: tauri::State<'_, HostState>,
    ssh: tauri::State<'_, SshState>,
    host_id: String,
    target: HostTarget,
    source: HerdrBinarySource,
    custom_path: Option<String>,
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
    let binary = select_host_binary(&info, source, custom_path.as_deref(), &directory)?;
    if source == HerdrBinarySource::Default {
        deploy_file(
            &target,
            Some(&ssh.0),
            &info.home,
            &directory,
            "herdr",
            &herdr_bytes,
            &manifest.herdr.sha256,
        )
        .await?;
    }
    // Recheck the actual chosen binary and every running Session before replacing
    // the helper connection or allowing the frontend to persist new paths.
    inspect_host_binary(&target, &ssh.0, &binary)
        .await?
        .require_compatible()?;
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
        artifact_identity: hash(&manifest_bytes),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn metadata_fixture(bytes: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("schema.json");
        std::fs::write(&document, bytes).unwrap();
        let binary = dir.path().join("herdr");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\ncat {}\n",
                shell_quote(document.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, binary.to_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn runtime_metadata_accepts_large_schema_up_to_shared_json_limit() {
        // HERDR 0.9.0 emits a 275,129-byte schema, beyond the 64 KiB probe limit.
        for size in [275_129, MAX_NDJSON_LINE_BYTES] {
            let json = format!("{{\"description\":\"{}\"}}", "x".repeat(size - 18));
            assert_eq!(json.len(), size);
            let (_dir, binary) = metadata_fixture(json.as_bytes());
            let schema = runtime_metadata(
                &HostTarget::Local,
                None,
                &binary,
                "default",
                "api schema --json",
            )
            .await
            .unwrap();
            assert_eq!(schema["description"].as_str().unwrap().len(), size - 18);
        }
    }

    #[tokio::test]
    async fn runtime_metadata_rejects_oversized_or_invalid_json() {
        for (bytes, error) in [
            (
                vec![b' '; MAX_NDJSON_LINE_BYTES + 1],
                "host-probe-output-too-large",
            ),
            (b"not json".to_vec(), "invalid-runtime-json:"),
        ] {
            let (_dir, binary) = metadata_fixture(&bytes);
            let result = runtime_metadata(
                &HostTarget::Local,
                None,
                &binary,
                "default",
                "api schema --json",
            )
            .await;
            assert!(result.unwrap_err().starts_with(error));
        }
    }

    #[tokio::test]
    async fn probes_keep_small_limit_and_runtime_requires_command_success() {
        let (_dir, binary) = metadata_fixture(&vec![b' '; MAX_PROBE_OUTPUT_BYTES]);
        assert_eq!(
            execute(
                &HostTarget::Local,
                None,
                &shell_quote(&binary).unwrap(),
                &[]
            )
            .await
            .unwrap_err(),
            "host-probe-output-too-large"
        );
        std::fs::write(&binary, "#!/bin/sh\nprintf '{}'\nexit 1\n").unwrap();
        assert_eq!(
            runtime_metadata(
                &HostTarget::Local,
                None,
                &binary,
                "default",
                "status --json"
            )
            .await
            .unwrap_err(),
            "host-setup-command-failed"
        );
    }

    #[test]
    fn host_source_is_explicit_and_missing_installed_does_not_fall_back() {
        let probe = HostProbe {
            os: "linux".into(),
            arch: "x86_64".into(),
            home: "/home/user".into(),
            installed_herdr: None,
        };
        assert_eq!(
            select_host_binary(&probe, HerdrBinarySource::Default, None, "candidate").unwrap(),
            "/home/user/.local/share/yuzora/runtimes/candidate/herdr"
        );
        assert!(select_host_binary(&probe, HerdrBinarySource::Global, None, "candidate").is_err());
        assert!(select_host_binary(
            &probe,
            HerdrBinarySource::Custom,
            Some("C:\\herdr.exe"),
            "candidate"
        )
        .is_err());
        assert_eq!(
            select_host_binary(
                &probe,
                HerdrBinarySource::Custom,
                Some("/opt/herdr"),
                "candidate"
            )
            .unwrap(),
            "/opt/herdr"
        );
    }
    #[test]
    fn deployment_rejects_a_stale_herdr_even_when_the_app_version_matches() {
        let mut manifest: Manifest = serde_json::from_value(serde_json::json!({
            "protocol": PROTOCOL_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "target": "linux-x86_64",
            "helper": {"path": "linux-x86_64/yuzora-host", "sha256": "a".repeat(64)},
            "herdr": {"path": "linux-x86_64/herdr", "sha256": "b".repeat(64), "version": "0.8.2", "protocol": 20}
        })).unwrap();
        assert_eq!(
            validate_manifest(&manifest, "linux-x86_64").unwrap_err(),
            "herdr-artifact-version-mismatch"
        );
        manifest.herdr.version = "0.9.0".into();
        manifest.herdr.protocol = 22;
        validate_manifest(&manifest, "linux-x86_64").unwrap();
    }

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
