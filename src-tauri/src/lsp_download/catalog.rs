// Immutable reviewed LSP catalog: every curated managed route binds to an
// exact version, URL/package spec, digest or lock identity, unpack kind, and
// expected executable. Missing or incomplete metadata is fail-closed.
//
// Digests are independent reviewed inputs (GitHub release asset digest, npm
// lock integrity, PyPI wheel sha256). Install-time hashes are never written
// back into this catalog.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

const EMBEDDED_MANIFEST: &str = include_str!("../../lsp-catalog/manifest.json");
const VTSLS_PACKAGE_JSON: &str = include_str!("../../lsp-catalog/npm/vtsls/package.json");
const VTSLS_PACKAGE_LOCK: &str = include_str!("../../lsp-catalog/npm/vtsls/package-lock.json");
const TLS_PACKAGE_JSON: &str =
    include_str!("../../lsp-catalog/npm/typescript-language-server/package.json");
const TLS_PACKAGE_LOCK: &str =
    include_str!("../../lsp-catalog/npm/typescript-language-server/package-lock.json");
const PYRIGHT_PACKAGE_JSON: &str = include_str!("../../lsp-catalog/npm/pyright/package.json");
const PYRIGHT_PACKAGE_LOCK: &str = include_str!("../../lsp-catalog/npm/pyright/package-lock.json");
const PYLSP_REQUIREMENTS: &str = include_str!("../../lsp-catalog/pip/pylsp/requirements.txt");
const VTSLS_CONTENT_MANIFEST: &str =
    include_str!("../../lsp-catalog/npm/vtsls/content-manifest.json");
const TLS_CONTENT_MANIFEST: &str =
    include_str!("../../lsp-catalog/npm/typescript-language-server/content-manifest.json");
const PYRIGHT_CONTENT_MANIFEST: &str =
    include_str!("../../lsp-catalog/npm/pyright/content-manifest.json");
const PYLSP_CONTENT_MANIFEST: &str =
    include_str!("../../lsp-catalog/pip/pylsp/content-manifest.json");

pub const CURATED_NPM_SERVER_IDS: &[&str] = &["vtsls", "typescript-language-server", "pyright"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspCatalog {
    pub version: String,
    servers: Vec<CatalogServer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogServer {
    language: String,
    server_id: String,
    kind: String,
    version: String,
    bin: String,
    packages: Vec<PinnedPackage>,
    package: Option<String>,
    package_dir: Option<String>,
    requirements_file: Option<String>,
    content_manifest: Option<String>,
    launch_target: Option<String>,
    allow_scripts: bool,
    allow_scripts_rationale: Option<String>,
    only_binary: bool,
    platforms: BTreeMap<String, RawPlatform>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PinnedPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlatform {
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    unpack: Option<String>,
    #[serde(default)]
    executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub unpack: String,
    pub executable: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmIdentity {
    pub version: String,
    pub bin: String,
    pub server_id: String,
    pub packages: Vec<PinnedPackage>,
    pub package_json: String,
    pub package_lock: String,
    pub content: ContentTreeManifest,
    pub launch_target: String,
    pub allow_scripts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipIdentity {
    pub version: String,
    pub bin: String,
    pub server_id: String,
    pub package: String,
    pub requirements: String,
    pub content: ContentTreeManifest,
    pub launch_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentTreeManifest {
    pub version: u32,
    pub roots: Vec<String>,
    pub file_count: usize,
    pub tree_sha256: String,
    #[serde(default)]
    pub artifact_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogIdentity {
    Binary(BinaryIdentity),
    Npm(NpmIdentity),
    Pip(PipIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProvenance {
    pub catalog_version: String,
    pub language: String,
    pub server_id: String,
    pub version: String,
    pub platform: String,
    pub artifact: serde_json::Value,
    pub installed_path: String,
    pub digest: String,
    pub verified_at: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCatalog {
    catalog_version: String,
    servers: Vec<RawServer>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawServer {
    language: String,
    server_id: String,
    kind: String,
    version: String,
    #[serde(default)]
    bin: String,
    #[serde(default)]
    packages: Vec<PinnedPackage>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    package_dir: Option<String>,
    #[serde(default)]
    requirements_file: Option<String>,
    #[serde(default)]
    content_manifest: Option<String>,
    #[serde(default)]
    launch_target: Option<String>,
    #[serde(default)]
    allow_scripts: bool,
    #[serde(default)]
    allow_scripts_rationale: Option<String>,
    #[serde(default = "default_true")]
    only_binary: bool,
    #[serde(default)]
    platforms: BTreeMap<String, RawPlatform>,
}

fn default_true() -> bool {
    true
}

pub fn embedded_catalog() -> Result<&'static LspCatalog, String> {
    static CATALOG: OnceLock<Result<LspCatalog, String>> = OnceLock::new();
    match CATALOG.get_or_init(|| parse_catalog(EMBEDDED_MANIFEST)) {
        Ok(catalog) => Ok(catalog),
        Err(error) => Err(error.clone()),
    }
}

pub fn parse_catalog(json: &str) -> Result<LspCatalog, String> {
    let raw: RawCatalog =
        serde_json::from_str(json).map_err(|e| format!("LSP catalog 無法解析：{e}"))?;
    if raw.catalog_version.trim().is_empty() {
        return Err("LSP catalog 缺少 catalogVersion".into());
    }
    if raw.servers.is_empty() {
        return Err("LSP catalog 沒有任何 server".into());
    }
    let servers = raw
        .servers
        .into_iter()
        .map(|server| {
            if server.allow_scripts
                && server
                    .allow_scripts_rationale
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                return Err(format!(
                    "{} / {} 啟用 lifecycle scripts 但缺少 reviewed rationale",
                    server.language, server.server_id
                ));
            }
            Ok(CatalogServer {
                language: server.language,
                server_id: server.server_id,
                kind: server.kind,
                version: server.version,
                bin: server.bin,
                packages: server.packages,
                package: server.package,
                package_dir: server.package_dir,
                requirements_file: server.requirements_file,
                content_manifest: server.content_manifest,
                launch_target: server.launch_target,
                allow_scripts: server.allow_scripts,
                allow_scripts_rationale: server.allow_scripts_rationale,
                only_binary: server.only_binary,
                platforms: server.platforms,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(LspCatalog {
        version: raw.catalog_version,
        servers,
    })
}

pub fn platform_key(os: &str, arch: &str) -> String {
    format!("{os}/{arch}")
}

pub fn resolve_identity(
    catalog: &LspCatalog,
    language: &str,
    server_id: &str,
    os: &str,
    arch: &str,
) -> Result<CatalogIdentity, String> {
    let server = catalog
        .servers
        .iter()
        .find(|s| s.language == language && s.server_id == server_id)
        .ok_or_else(|| {
            format!("缺少已審核的完整性 metadata，拒絕安裝 {server_id}（{language}）")
        })?;
    if server.version.trim().is_empty() {
        return Err(format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {server_id}（未釘選版本）"
        ));
    }
    let platform = resolve_platform(server, os, arch)?;
    match server.kind.as_str() {
        "binary" => Ok(CatalogIdentity::Binary(binary_identity(server, platform)?)),
        "npm" => Ok(CatalogIdentity::Npm(npm_identity(server, platform)?)),
        "pip" => Ok(CatalogIdentity::Pip(pip_identity(server, platform)?)),
        other => Err(format!("{server_id} 的 catalog kind `{other}` 不受支援")),
    }
}

fn resolve_platform<'a>(
    server: &'a CatalogServer,
    os: &str,
    arch: &str,
) -> Result<&'a RawPlatform, String> {
    let key = platform_key(os, arch);
    let platform = server
        .platforms
        .get(&key)
        .or_else(|| server.platforms.get("*"))
        .ok_or_else(|| {
            format!(
                "缺少已審核的完整性 metadata，拒絕安裝 {}（{key}）",
                server.server_id
            )
        })?;
    if platform.status != "supported" {
        return Err(platform.reason.clone().unwrap_or_else(|| {
            format!(
                "{} 在 {key} 不可用（catalog 標記 unavailable）",
                server.server_id
            )
        }));
    }
    Ok(platform)
}

fn binary_identity(
    server: &CatalogServer,
    platform: &RawPlatform,
) -> Result<BinaryIdentity, String> {
    let url = require_field(&platform.url, server, "url")?;
    let sha256 = require_sha256(&platform.sha256, server)?;
    let unpack = require_field(&platform.unpack, server, "unpack")?;
    let executable = require_field(&platform.executable, server, "executable")?;
    if !matches!(unpack.as_str(), "gz" | "bare" | "zip") {
        return Err(format!(
            "{} 的 unpack `{unpack}` 不受支援",
            server.server_id
        ));
    }
    if !(url.starts_with("https://") || url.starts_with("http://127.0.0.1/")) {
        return Err(format!("{} 的下載 URL 必須是 https", server.server_id));
    }
    Ok(BinaryIdentity {
        version: server.version.clone(),
        url,
        sha256,
        unpack,
        executable,
    })
}

fn npm_identity(server: &CatalogServer, _platform: &RawPlatform) -> Result<NpmIdentity, String> {
    if server.packages.is_empty() || server.bin.trim().is_empty() {
        return Err(format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（npm packages/bin）",
            server.server_id
        ));
    }
    if server
        .packages
        .iter()
        .any(|p| p.name.is_empty() || p.version.is_empty())
    {
        return Err(format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（未釘選 npm 版本）",
            server.server_id
        ));
    }
    let dir = server.package_dir.as_deref().ok_or_else(|| {
        format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（packageDir）",
            server.server_id
        )
    })?;
    let package_json = embedded_text(&format!("{dir}/package.json"))?;
    let package_lock = embedded_text(&format!("{dir}/package-lock.json"))?;
    let content = embedded_content_manifest(server)?;
    let launch_target = reviewed_launch_target(server)?;
    if !lockfile_is_immutable(&package_lock) {
        return Err(format!(
            "{} 的 package-lock.json 缺少 reviewed integrity",
            server.server_id
        ));
    }
    Ok(NpmIdentity {
        version: server.version.clone(),
        bin: server.bin.clone(),
        server_id: server.server_id.clone(),
        packages: server.packages.clone(),
        package_json,
        package_lock,
        content,
        launch_target,
        allow_scripts: server.allow_scripts,
    })
}

fn pip_identity(server: &CatalogServer, _platform: &RawPlatform) -> Result<PipIdentity, String> {
    if !server.only_binary {
        return Err(format!(
            "{} 禁止非 binary wheel 的 pip 安裝",
            server.server_id
        ));
    }
    let package = server
        .package
        .clone()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            format!(
                "缺少已審核的完整性 metadata，拒絕安裝 {}（pip package）",
                server.server_id
            )
        })?;
    let req_rel = server.requirements_file.as_deref().ok_or_else(|| {
        format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（requirementsFile）",
            server.server_id
        )
    })?;
    let requirements = embedded_text(req_rel)?;
    let content = embedded_content_manifest(server)?;
    let launch_target = reviewed_launch_target(server)?;
    if !requirements_have_hashes(&requirements) {
        return Err(format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（pip hashes）",
            server.server_id
        ));
    }
    let artifact_sha256 = content.artifact_sha256.as_deref().ok_or_else(|| {
        format!(
            "{} 的 pip content manifest 缺少 wheel digest",
            server.server_id
        )
    })?;
    if !is_sha256_hex(artifact_sha256)
        || !requirements.contains(&format!("--hash=sha256:{artifact_sha256}"))
    {
        return Err(format!(
            "{} 的 pip content manifest 未綁定 reviewed wheel hash",
            server.server_id
        ));
    }
    if server.bin.trim().is_empty() {
        return Err(format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（pip bin）",
            server.server_id
        ));
    }
    Ok(PipIdentity {
        version: server.version.clone(),
        bin: server.bin.clone(),
        server_id: server.server_id.clone(),
        package,
        requirements,
        content,
        launch_target,
    })
}

fn embedded_content_manifest(server: &CatalogServer) -> Result<ContentTreeManifest, String> {
    let rel = server.content_manifest.as_deref().ok_or_else(|| {
        format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（contentManifest）",
            server.server_id
        )
    })?;
    let raw = embedded_text(rel)?;
    let manifest: ContentTreeManifest = serde_json::from_str(&raw)
        .map_err(|e| format!("{} 的 content manifest 無法解析：{e}", server.server_id))?;
    if manifest.version != 1
        || manifest.roots.is_empty()
        || manifest.file_count == 0
        || !is_sha256_hex(&manifest.tree_sha256)
        || manifest
            .roots
            .iter()
            .any(|root| !safe_manifest_relative(root))
    {
        return Err(format!("{} 的 content manifest 無效", server.server_id));
    }
    Ok(manifest)
}

fn reviewed_launch_target(server: &CatalogServer) -> Result<String, String> {
    let target = server
        .launch_target
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty() && safe_manifest_relative(target))
        .ok_or_else(|| {
            format!(
                "缺少已審核的完整性 metadata，拒絕安裝 {}（launchTarget）",
                server.server_id
            )
        })?;
    Ok(target.to_string())
}

fn safe_manifest_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '/' | '@' | '.' | '_' | '-')
        })
        && Path::new(value)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn require_field(
    value: &Option<String>,
    server: &CatalogServer,
    field: &str,
) -> Result<String, String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "缺少已審核的完整性 metadata，拒絕安裝 {}（{field}）",
                server.server_id
            )
        })
}

fn require_sha256(value: &Option<String>, server: &CatalogServer) -> Result<String, String> {
    let sha = require_field(value, server, "sha256")?;
    if !is_sha256_hex(&sha) {
        return Err(format!(
            "缺少已審核的完整性 metadata，拒絕安裝 {}（sha256 格式無效）",
            server.server_id
        ));
    }
    Ok(sha.to_ascii_lowercase())
}

pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn lockfile_is_immutable(lock: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(lock) else {
        return false;
    };
    let Some(packages) = value.get("packages").and_then(|p| p.as_object()) else {
        return false;
    };
    packages.iter().all(|(name, pkg)| {
        name.is_empty()
            || pkg.get("link").and_then(|v| v.as_bool()).unwrap_or(false)
            || pkg
                .get("integrity")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
    })
}

fn requirements_have_hashes(text: &str) -> bool {
    let mut saw_requirement = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.contains("==") {
            saw_requirement = true;
        }
        if trimmed.contains("==") && !text.contains("--hash=sha256:") {
            return false;
        }
    }
    saw_requirement && text.contains("--hash=sha256:")
}

fn embedded_text(rel: &str) -> Result<String, String> {
    match rel {
        "npm/vtsls/package.json" => Ok(VTSLS_PACKAGE_JSON.to_string()),
        "npm/vtsls/package-lock.json" => Ok(VTSLS_PACKAGE_LOCK.to_string()),
        "npm/typescript-language-server/package.json" => Ok(TLS_PACKAGE_JSON.to_string()),
        "npm/typescript-language-server/package-lock.json" => Ok(TLS_PACKAGE_LOCK.to_string()),
        "npm/pyright/package.json" => Ok(PYRIGHT_PACKAGE_JSON.to_string()),
        "npm/pyright/package-lock.json" => Ok(PYRIGHT_PACKAGE_LOCK.to_string()),
        "pip/pylsp/requirements.txt" => Ok(PYLSP_REQUIREMENTS.to_string()),
        "npm/vtsls/content-manifest.json" => Ok(VTSLS_CONTENT_MANIFEST.to_string()),
        "npm/typescript-language-server/content-manifest.json" => {
            Ok(TLS_CONTENT_MANIFEST.to_string())
        }
        "npm/pyright/content-manifest.json" => Ok(PYRIGHT_CONTENT_MANIFEST.to_string()),
        "pip/pylsp/content-manifest.json" => Ok(PYLSP_CONTENT_MANIFEST.to_string()),
        _ => Err(format!("catalog sidecar `{rel}` 未嵌入")),
    }
}

pub fn require_binary(
    language: &str,
    server_id: &str,
    os: &str,
    arch: &str,
) -> Result<BinaryIdentity, String> {
    match resolve_identity(embedded_catalog()?, language, server_id, os, arch)? {
        CatalogIdentity::Binary(identity) => Ok(identity),
        _ => Err(format!("{server_id} 不是 binary catalog route")),
    }
}

pub fn require_npm(
    language: &str,
    server_id: &str,
    os: &str,
    arch: &str,
) -> Result<NpmIdentity, String> {
    match resolve_identity(embedded_catalog()?, language, server_id, os, arch)? {
        CatalogIdentity::Npm(identity) => Ok(identity),
        _ => Err(format!("{server_id} 不是 npm catalog route")),
    }
}

pub fn require_pip(
    language: &str,
    server_id: &str,
    os: &str,
    arch: &str,
) -> Result<PipIdentity, String> {
    match resolve_identity(embedded_catalog()?, language, server_id, os, arch)? {
        CatalogIdentity::Pip(identity) => Ok(identity),
        _ => Err(format!("{server_id} 不是 pip catalog route")),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀取 {} 失敗：{e}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

pub fn sha256_matches(actual_hex: &str, expected_hex: &str) -> bool {
    actual_hex.eq_ignore_ascii_case(expected_hex)
}

pub fn provenance_path(installed: &Path) -> PathBuf {
    let mut name = installed
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "server".into());
    name.push(".provenance.json");
    installed.with_file_name(name)
}

pub fn artifact_for(identity: &CatalogIdentity) -> serde_json::Value {
    match identity {
        CatalogIdentity::Binary(identity) => serde_json::json!({
            "kind": "binary",
            "url": identity.url,
            "sha256": identity.sha256,
            "unpack": identity.unpack,
            "executable": identity.executable,
        }),
        CatalogIdentity::Npm(identity) => serde_json::json!({
            "kind": "npm",
            "packages": identity.packages,
            "lockSha256": sha256_hex(identity.package_lock.as_bytes()),
            "treeSha256": identity.content.tree_sha256,
            "launchTarget": identity.launch_target,
            "allowScripts": identity.allow_scripts,
        }),
        CatalogIdentity::Pip(identity) => serde_json::json!({
            "kind": "pip",
            "package": identity.package,
            "requirementsSha256": sha256_hex(identity.requirements.as_bytes()),
            "treeSha256": identity.content.tree_sha256,
            "launchTarget": identity.launch_target,
        }),
    }
}

pub fn write_provenance(
    catalog: &LspCatalog,
    language: &str,
    server_id: &str,
    os: &str,
    arch: &str,
    installed: &Path,
    identity: &CatalogIdentity,
) -> Result<InstallProvenance, String> {
    let digest = sha256_file(installed)?;
    let record = InstallProvenance {
        catalog_version: catalog.version.clone(),
        language: language.to_string(),
        server_id: server_id.to_string(),
        version: match identity {
            CatalogIdentity::Binary(v) => v.version.clone(),
            CatalogIdentity::Npm(v) => v.version.clone(),
            CatalogIdentity::Pip(v) => v.version.clone(),
        },
        platform: platform_key(os, arch),
        artifact: artifact_for(identity),
        installed_path: installed.to_string_lossy().into_owned(),
        digest,
        verified_at: chrono::Utc::now().to_rfc3339(),
    };
    let path = provenance_path(installed);
    let json = serde_json::to_string_pretty(&record)
        .map_err(|e| format!("serialize provenance 失敗：{e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("寫入 {} 失敗：{e}", path.display()))?;
    Ok(record)
}

pub fn verify_installed(
    catalog: &LspCatalog,
    language: &str,
    server_id: &str,
    installed: &Path,
    os: &str,
    arch: &str,
) -> Result<InstallProvenance, String> {
    let identity = resolve_identity(catalog, language, server_id, os, arch)?;
    if !installed.is_file() {
        return Err(format!(
            "已安裝的 {server_id} 不存在，請重新安裝（{}）",
            installed.display()
        ));
    }
    match &identity {
        CatalogIdentity::Binary(binary) => {
            let digest = sha256_file(installed)?;
            if !sha256_matches(&digest, &binary.sha256) {
                return Err(format!(
                    "已安裝的 {server_id} 檔案摘要不符，請重新安裝（預期 {}，實得 {digest}）",
                    binary.sha256
                ));
            }
        }
        CatalogIdentity::Npm(npm) => verify_npm_install(installed, npm)?,
        CatalogIdentity::Pip(pip) => verify_pip_install(installed, pip)?,
    }
    // Sidecar provenance is informational only. Catalog identity is the root
    // authority; a rewritten sidecar cannot bless a tampered install.
    let digest = sha256_file(installed).unwrap_or_default();
    let expected_version = match &identity {
        CatalogIdentity::Binary(v) => v.version.as_str(),
        CatalogIdentity::Npm(v) => v.version.as_str(),
        CatalogIdentity::Pip(v) => v.version.as_str(),
    };
    Ok(InstallProvenance {
        catalog_version: catalog.version.clone(),
        language: language.to_string(),
        server_id: server_id.to_string(),
        version: expected_version.to_string(),
        platform: platform_key(os, arch),
        artifact: artifact_for(&identity),
        installed_path: installed.to_string_lossy().into_owned(),
        digest,
        verified_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn npm_package_root(installed: &Path) -> Result<PathBuf, String> {
    let parent = installed.parent().ok_or_else(|| {
        format!(
            "已安裝的 npm 啟動器路徑無效，請重新安裝（{}）",
            installed.display()
        )
    })?;
    if parent.file_name().and_then(|name| name.to_str()) == Some(".bin") {
        if let Some(node_modules) = parent.parent() {
            if node_modules.file_name().and_then(|name| name.to_str()) == Some("node_modules") {
                if let Some(root) = node_modules.parent() {
                    return Ok(root.to_path_buf());
                }
            }
        }
    }
    Err(format!(
        "已安裝的 npm 啟動器不在 reviewed prefix 內，請重新安裝（{}）",
        installed.display()
    ))
}

fn npm_launcher_bytes(identity: &NpmIdentity, windows: bool) -> Vec<u8> {
    if windows {
        format!(
            "@echo off\r\nnode \"%~dp0\\..\\{}\" %*\r\n",
            identity.launch_target.replace('/', "\\")
        )
        .into_bytes()
    } else {
        format!(
            "#!/bin/sh\nexec node \"$(dirname \"$0\")/../{}\" \"$@\"\n",
            identity.launch_target
        )
        .into_bytes()
    }
}

fn pip_launcher_bytes(identity: &PipIdentity, windows: bool) -> Vec<u8> {
    if windows {
        format!(
            "@echo off\r\n\"%~dp0python.exe\" -m {} %*\r\n",
            identity.launch_target
        )
        .into_bytes()
    } else {
        format!(
            "#!/bin/sh\nexec \"$(dirname \"$0\")/python\" -m {} \"$@\"\n",
            identity.launch_target
        )
        .into_bytes()
    }
}

fn write_managed_launcher(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes)
        .map_err(|e| format!("寫入 managed LSP launcher {} 失敗：{e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("設定 managed LSP launcher 權限失敗：{e}"))?;
    }
    Ok(())
}

pub(crate) fn write_npm_launcher(installed: &Path, identity: &NpmIdentity) -> Result<(), String> {
    write_managed_launcher(installed, &npm_launcher_bytes(identity, cfg!(windows)))
}

pub(crate) fn write_pip_launcher(installed: &Path, identity: &PipIdentity) -> Result<(), String> {
    write_managed_launcher(installed, &pip_launcher_bytes(identity, cfg!(windows)))
}

fn verify_launcher(path: &Path, expected: &[u8], server_id: &str) -> Result<(), String> {
    let actual = std::fs::read(path)
        .map_err(|_| format!("已安裝的 {server_id} 缺少 reviewed launcher，請重新安裝"))?;
    if actual != expected {
        return Err(format!(
            "已安裝的 {server_id} launcher 內容與 catalog 不符，請重新安裝"
        ));
    }
    Ok(())
}

fn collect_tree_files(
    base: &Path,
    current: &Path,
    files: &mut Vec<(String, u64, String)>,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(current)
        .map_err(|e| format!("讀取 managed LSP tree {} 失敗：{e}", current.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "managed LSP tree 含未審核 symlink：{}",
            current.display()
        ));
    }
    if metadata.is_file() {
        let relative = current
            .strip_prefix(base)
            .map_err(|_| "managed LSP tree 路徑逸出".to_string())?;
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "managed LSP tree 路徑不是 UTF-8".to_string())?
            .join("/");
        files.push((relative, metadata.len(), sha256_file(current)?));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "managed LSP tree 含非普通項目：{}",
            current.display()
        ));
    }
    for entry in std::fs::read_dir(current)
        .map_err(|e| format!("讀取 managed LSP tree {} 失敗：{e}", current.display()))?
    {
        let entry = entry.map_err(|e| format!("讀取 managed LSP tree entry 失敗：{e}"))?;
        collect_tree_files(base, &entry.path(), files)?;
    }
    Ok(())
}

fn calculate_content_tree(base: &Path, roots: &[String]) -> Result<ContentTreeManifest, String> {
    let mut files = Vec::new();
    for root in roots {
        if !safe_manifest_relative(root) {
            return Err("managed LSP content root 無效".to_string());
        }
        collect_tree_files(base, &base.join(root), &mut files)?;
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (path, size, digest) in &files {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(size.to_string().as_bytes());
        hasher.update([0]);
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    Ok(ContentTreeManifest {
        version: 1,
        roots: roots.to_vec(),
        file_count: files.len(),
        tree_sha256: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        artifact_sha256: None,
    })
}

fn verify_content_tree(
    base: &Path,
    expected: &ContentTreeManifest,
    server_id: &str,
) -> Result<(), String> {
    let actual = calculate_content_tree(base, &expected.roots)?;
    if actual.file_count != expected.file_count
        || !sha256_matches(&actual.tree_sha256, &expected.tree_sha256)
    {
        return Err(format!(
            "已安裝的 {server_id} package/module tree 與 catalog 不符，請重新安裝"
        ));
    }
    Ok(())
}

fn verify_npm_install(installed: &Path, identity: &NpmIdentity) -> Result<(), String> {
    verify_launcher(
        installed,
        &npm_launcher_bytes(identity, cfg!(windows)),
        &identity.server_id,
    )?;
    let root = npm_package_root(installed)?;
    let package_json = std::fs::read_to_string(root.join("package.json")).map_err(|_| {
        format!(
            "已安裝的 {} 缺少 package.json，請重新安裝",
            identity.server_id
        )
    })?;
    let package_lock = std::fs::read_to_string(root.join("package-lock.json")).map_err(|_| {
        format!(
            "已安裝的 {} 缺少 package-lock.json，請重新安裝",
            identity.server_id
        )
    })?;
    if package_json != identity.package_json
        || !sha256_matches(
            &sha256_hex(package_lock.as_bytes()),
            &sha256_hex(identity.package_lock.as_bytes()),
        )
    {
        return Err(format!(
            "已安裝的 {} lock/package 身分與目前 catalog 不符，請重新安裝",
            identity.server_id
        ));
    }
    for package in &identity.packages {
        let mut meta = root.join("node_modules");
        for part in package.name.split('/') {
            meta.push(part);
        }
        meta.push("package.json");
        let text = std::fs::read_to_string(&meta).map_err(|_| {
            format!(
                "已安裝的 {} 缺少套件 {}，請重新安裝",
                identity.server_id, package.name
            )
        })?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
            format!(
                "已安裝的 {} 套件 {} metadata 無法解析，請重新安裝",
                identity.server_id, package.name
            )
        })?;
        let name = value.get("name").and_then(|value| value.as_str());
        let version = value.get("version").and_then(|value| value.as_str());
        if name != Some(package.name.as_str()) || version != Some(package.version.as_str()) {
            return Err(format!(
                "已安裝的 {} 套件 {} 版本與目前 catalog 不符，請重新安裝",
                identity.server_id, package.name
            ));
        }
    }
    verify_content_tree(
        &root.join("node_modules"),
        &identity.content,
        &identity.server_id,
    )?;
    Ok(())
}

fn pip_venv_root(installed: &Path) -> Result<PathBuf, String> {
    let parent = installed.parent().ok_or_else(|| {
        format!(
            "已安裝的 pip 啟動器路徑無效，請重新安裝（{}）",
            installed.display()
        )
    })?;
    match parent.file_name().and_then(|name| name.to_str()) {
        Some("bin" | "Scripts") => parent.parent().map(Path::to_path_buf).ok_or_else(|| {
            format!(
                "已安裝的 pip venv 無效，請重新安裝（{}）",
                installed.display()
            )
        }),
        _ => Err(format!(
            "已安裝的 pip 啟動器不在 reviewed venv 內，請重新安裝（{}）",
            installed.display()
        )),
    }
}

fn pip_site_packages(venv: &Path) -> Result<PathBuf, String> {
    let unix = venv.join("lib");
    if unix.is_dir() {
        for entry in std::fs::read_dir(&unix).map_err(|_| {
            format!(
                "已安裝的 pip site-packages 無法讀取，請重新安裝（{}）",
                unix.display()
            )
        })? {
            let entry =
                entry.map_err(|_| "已安裝的 pip site-packages 無法讀取，請重新安裝".to_string())?;
            let candidate = entry.path().join("site-packages");
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
    }
    let windows = venv.join("Lib").join("site-packages");
    if windows.is_dir() {
        return Ok(windows);
    }
    Err(format!(
        "已安裝的 pip site-packages 不存在，請重新安裝（{}）",
        venv.display()
    ))
}

fn verify_pip_install(installed: &Path, identity: &PipIdentity) -> Result<(), String> {
    verify_launcher(
        installed,
        &pip_launcher_bytes(identity, cfg!(windows)),
        &identity.server_id,
    )?;
    let venv = pip_venv_root(installed)?;
    let requirements_path = venv.join("requirements.txt");
    let requirements = std::fs::read_to_string(&requirements_path).map_err(|_| {
        format!(
            "已安裝的 {} 缺少 requirements.txt，請重新安裝",
            identity.server_id
        )
    })?;
    if !sha256_matches(
        &sha256_hex(requirements.as_bytes()),
        &sha256_hex(identity.requirements.as_bytes()),
    ) {
        return Err(format!(
            "已安裝的 {} requirements 與目前 catalog 不符，請重新安裝",
            identity.server_id
        ));
    }
    let site = pip_site_packages(&venv)?;
    let expected_version = identity.version.as_str();
    let mut found = false;
    let prefix = format!("{}-", identity.package.replace('-', "_"));
    let prefix_dash = format!("{}-", identity.package);
    for entry in std::fs::read_dir(&site).map_err(|_| {
        format!(
            "已安裝的 {} site-packages 無法讀取，請重新安裝",
            identity.server_id
        )
    })? {
        let entry = entry.map_err(|_| {
            format!(
                "已安裝的 {} site-packages 無法讀取，請重新安裝",
                identity.server_id
            )
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.ends_with(".dist-info")
            && (name.starts_with(&prefix) || name.starts_with(&prefix_dash)))
        {
            continue;
        }
        let metadata = std::fs::read_to_string(entry.path().join("METADATA")).map_err(|_| {
            format!(
                "已安裝的 {} 缺少套件 METADATA，請重新安裝",
                identity.server_id
            )
        })?;
        let mut meta_name = None;
        let mut meta_version = None;
        for line in metadata.lines() {
            if let Some(value) = line.strip_prefix("Name: ") {
                meta_name = Some(value.trim().to_string());
            }
            if let Some(value) = line.strip_prefix("Version: ") {
                meta_version = Some(value.trim().to_string());
            }
        }
        if meta_name.as_deref() == Some(identity.package.as_str())
            && meta_version.as_deref() == Some(expected_version)
        {
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!(
            "已安裝的 {} 套件 {}=={} 與目前 catalog 不符，請重新安裝",
            identity.server_id, identity.package, expected_version
        ));
    }
    verify_content_tree(&site, &identity.content, &identity.server_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::plan::route_for;
    use super::*;
    use crate::lsp_adapters;

    fn fixture_catalog(extra_server: serde_json::Value) -> LspCatalog {
        let mut catalog = serde_json::json!({
            "catalogVersion": "test-1",
            "servers": []
        });
        catalog["servers"]
            .as_array_mut()
            .unwrap()
            .push(extra_server);
        parse_catalog(&catalog.to_string()).unwrap()
    }

    fn npm_content_fixture() -> (tempfile::TempDir, PathBuf, NpmIdentity, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("npm").join("vtsls");
        let installed = prefix.join("node_modules").join(".bin").join("vtsls");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        let mut identity = require_npm(
            "typescript",
            "vtsls",
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap();
        std::fs::write(prefix.join("package.json"), &identity.package_json).unwrap();
        std::fs::write(prefix.join("package-lock.json"), &identity.package_lock).unwrap();
        let package = prefix
            .join("node_modules")
            .join("@vtsls")
            .join("language-server");
        std::fs::create_dir_all(package.join("bin")).unwrap();
        std::fs::write(
            package.join("package.json"),
            serde_json::json!({
                "name": "@vtsls/language-server",
                "version": identity.packages[0].version,
            })
            .to_string(),
        )
        .unwrap();
        let module = package.join("bin").join("vtsls.js");
        std::fs::write(&module, "console.log('reviewed');\n").unwrap();
        identity.content.roots = vec!["@vtsls/language-server".to_string()];
        write_npm_launcher(&installed, &identity).unwrap();
        identity.content =
            calculate_content_tree(&prefix.join("node_modules"), &identity.content.roots).unwrap();
        verify_npm_install(&installed, &identity).unwrap();
        (tmp, installed, identity, module)
    }

    fn pip_content_fixture() -> (tempfile::TempDir, PathBuf, PipIdentity, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join("pyenv");
        let installed = venv.join("bin").join("pylsp");
        let site = venv.join("lib").join("python3.12").join("site-packages");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&site).unwrap();
        let mut identity = PipIdentity {
            version: "1.15.0".into(),
            bin: "pylsp".into(),
            server_id: "pylsp".into(),
            package: "python-lsp-server".into(),
            requirements: format!(
                "python-lsp-server==1.15.0 --hash=sha256:{}\n",
                "0".repeat(64)
            ),
            content: ContentTreeManifest {
                version: 1,
                roots: vec!["pylsp".into()],
                file_count: 1,
                tree_sha256: "0".repeat(64),
                artifact_sha256: None,
            },
            launch_target: "pylsp".into(),
        };
        std::fs::write(venv.join("requirements.txt"), &identity.requirements).unwrap();
        let dist = site.join(format!("python_lsp_server-{}.dist-info", identity.version));
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(
            dist.join("METADATA"),
            format!(
                "Name: {}\nVersion: {}\n",
                identity.package, identity.version
            ),
        )
        .unwrap();
        let package = site.join("pylsp");
        std::fs::create_dir_all(&package).unwrap();
        let module = package.join("__main__.py");
        std::fs::write(&module, "def main():\n    return 0\n").unwrap();
        write_pip_launcher(&installed, &identity).unwrap();
        identity.content = calculate_content_tree(&site, &identity.content.roots).unwrap();
        verify_pip_install(&installed, &identity).unwrap();
        (tmp, installed, identity, module)
    }

    #[test]
    fn embedded_catalog_parses_and_covers_every_curated_adapter() {
        let catalog = embedded_catalog().expect("embedded catalog must parse");
        assert_eq!(catalog.version, "1");
        for lang in lsp_adapters::all() {
            for opt in lang.options {
                assert!(
                    route_for(lang.language, opt.id).is_ok(),
                    "missing install route for {}/{}",
                    lang.language,
                    opt.id
                );
                let resolved = resolve_identity(
                    catalog,
                    lang.language,
                    opt.id,
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                );
                assert!(
                    resolved.is_ok()
                        || resolved.as_ref().err().is_some_and(|e| e.contains("不可用")
                            || e.contains("unavailable")
                            || e.contains("缺少已審核")
                            || e.contains("no official")),
                    "{}/{} must be supported or explicit fail-closed, got {resolved:?}",
                    lang.language,
                    opt.id
                );
            }
        }
    }

    #[test]
    fn missing_sha256_is_fail_closed() {
        let catalog = fixture_catalog(serde_json::json!({
            "language": "rust",
            "serverId": "rust-analyzer",
            "kind": "binary",
            "version": "2026-06-29",
            "bin": "rust-analyzer",
            "platforms": {
                "macos/aarch64": {
                    "status": "supported",
                    "url": "https://example.com/ra.gz",
                    "unpack": "gz",
                    "executable": "rust-analyzer"
                }
            }
        }));
        let err =
            resolve_identity(&catalog, "rust", "rust-analyzer", "macos", "aarch64").unwrap_err();
        assert!(err.contains("缺少已審核"), "{err}");
        assert!(err.contains("sha256"), "{err}");
    }

    #[test]
    fn digest_mismatch_is_rejected() {
        assert!(!sha256_matches(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "00"
        ));
        assert!(sha256_matches(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        ));
    }

    #[test]
    fn unavailable_platform_is_explicit() {
        let err = require_binary("markdown", "markdown-oxide", "windows", "aarch64").unwrap_err();
        assert!(
            err.contains("no official") || err.contains("不可用"),
            "{err}"
        );
    }

    #[test]
    fn embedded_pylsp_route_is_unavailable_pending_complete_dependency_manifests() {
        let error = require_pip(
            "python",
            "pylsp",
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap_err();

        assert!(error.contains("unavailable"), "{error}");
        assert!(
            error.contains("complete reviewed dependency content manifests"),
            "{error}"
        );
    }

    fn binary_fixture(bytes: &[u8]) -> (LspCatalog, String) {
        let digest = sha256_hex(bytes);
        let catalog = fixture_catalog(serde_json::json!({
            "language": "rust",
            "serverId": "rust-analyzer",
            "kind": "binary",
            "version": "test",
            "bin": "rust-analyzer",
            "platforms": {
                "*": {
                    "status": "supported",
                    "url": "https://example.com/ra",
                    "sha256": digest,
                    "unpack": "bare",
                    "executable": "rust-analyzer"
                }
            }
        }));
        (catalog, digest)
    }

    #[test]
    fn binary_verify_uses_catalog_digest_not_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = tmp.path().join("rust-analyzer");
        let bytes = b"verified-bytes";
        std::fs::write(&installed, bytes).unwrap();
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let (catalog, digest) = binary_fixture(bytes);
        let identity = resolve_identity(&catalog, "rust", "rust-analyzer", os, arch).unwrap();
        let record = write_provenance(
            &catalog,
            "rust",
            "rust-analyzer",
            os,
            arch,
            &installed,
            &identity,
        )
        .unwrap();
        assert_eq!(record.digest, digest);
        verify_installed(&catalog, "rust", "rust-analyzer", &installed, os, arch).unwrap();
        std::fs::write(&installed, b"tampered").unwrap();
        let tampered =
            verify_installed(&catalog, "rust", "rust-analyzer", &installed, os, arch).unwrap_err();
        assert!(tampered.contains("請重新安裝"), "{tampered}");
        assert!(tampered.contains("摘要不符"), "{tampered}");
    }

    #[test]
    fn binary_verify_rejects_matching_sidecar_and_tampered_install() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = tmp.path().join("rust-analyzer");
        let honest = b"catalog-bytes";
        std::fs::write(&installed, honest).unwrap();
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let (catalog, _) = binary_fixture(honest);
        let identity = resolve_identity(&catalog, "rust", "rust-analyzer", os, arch).unwrap();
        write_provenance(
            &catalog,
            "rust",
            "rust-analyzer",
            os,
            arch,
            &installed,
            &identity,
        )
        .unwrap();
        let tampered = b"attacker-bytes";
        std::fs::write(&installed, tampered).unwrap();
        let mut sidecar: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(provenance_path(&installed)).unwrap())
                .unwrap();
        sidecar["digest"] = serde_json::Value::String(sha256_hex(tampered));
        std::fs::write(
            provenance_path(&installed),
            serde_json::to_string_pretty(&sidecar).unwrap(),
        )
        .unwrap();
        let err =
            verify_installed(&catalog, "rust", "rust-analyzer", &installed, os, arch).unwrap_err();
        assert!(err.contains("摘要不符"), "{err}");
    }

    #[test]
    fn missing_sidecar_still_verifies_catalog_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = tmp.path().join("rust-analyzer");
        let bytes = b"catalog-only";
        std::fs::write(&installed, bytes).unwrap();
        let (catalog, _) = binary_fixture(bytes);
        verify_installed(
            &catalog,
            "rust",
            "rust-analyzer",
            &installed,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap();
    }

    #[test]
    fn npm_verify_rejects_tampered_lock_even_if_sidecar_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("npm").join("vtsls");
        let bin_dir = prefix.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let installed = bin_dir.join("vtsls");
        std::fs::write(&installed, b"bin").unwrap();
        let mut identity = require_npm(
            "typescript",
            "vtsls",
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap();
        std::fs::write(prefix.join("package.json"), &identity.package_json).unwrap();
        std::fs::write(prefix.join("package-lock.json"), &identity.package_lock).unwrap();
        let pkg_dir = prefix
            .join("node_modules")
            .join("@vtsls")
            .join("language-server");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            serde_json::json!({
                "name": "@vtsls/language-server",
                "version": identity.packages[0].version,
            })
            .to_string(),
        )
        .unwrap();
        identity.content.roots = vec!["@vtsls/language-server".to_string()];
        write_npm_launcher(&installed, &identity).unwrap();
        identity.content =
            calculate_content_tree(&prefix.join("node_modules"), &identity.content.roots).unwrap();
        verify_npm_install(&installed, &identity).unwrap();
        let catalog = embedded_catalog().unwrap();

        std::fs::write(prefix.join("package-lock.json"), "{ \"tampered\": true }").unwrap();
        write_provenance(
            catalog,
            "typescript",
            "vtsls",
            std::env::consts::OS,
            std::env::consts::ARCH,
            &installed,
            &CatalogIdentity::Npm(identity),
        )
        .unwrap();
        let mut sidecar: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(provenance_path(&installed)).unwrap())
                .unwrap();
        sidecar["digest"] = serde_json::Value::String(sha256_file(&installed).unwrap());
        std::fs::write(
            provenance_path(&installed),
            serde_json::to_string_pretty(&sidecar).unwrap(),
        )
        .unwrap();
        let err = verify_installed(
            catalog,
            "typescript",
            "vtsls",
            &installed,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap_err();
        assert!(
            err.contains("lock/package") || err.contains("請重新安裝"),
            "{err}"
        );
    }

    #[test]
    fn npm_verify_rejects_tampered_installed_package_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("npm").join("vtsls");
        let bin_dir = prefix.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let installed = bin_dir.join("vtsls");
        std::fs::write(&installed, b"bin").unwrap();
        let identity = require_npm(
            "typescript",
            "vtsls",
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap();
        std::fs::write(prefix.join("package.json"), &identity.package_json).unwrap();
        std::fs::write(prefix.join("package-lock.json"), &identity.package_lock).unwrap();
        let pkg_dir = prefix
            .join("node_modules")
            .join("@vtsls")
            .join("language-server");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            serde_json::json!({
                "name": "@vtsls/language-server",
                "version": "0.0.0-tampered",
            })
            .to_string(),
        )
        .unwrap();
        let catalog = embedded_catalog().unwrap();
        write_provenance(
            catalog,
            "typescript",
            "vtsls",
            std::env::consts::OS,
            std::env::consts::ARCH,
            &installed,
            &CatalogIdentity::Npm(identity),
        )
        .unwrap();
        let err = verify_installed(
            catalog,
            "typescript",
            "vtsls",
            &installed,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap_err();
        assert!(err.contains("版本") || err.contains("請重新安裝"), "{err}");
    }

    #[test]
    fn pip_verifier_rejects_tampered_requirements() {
        let (_tmp, installed, identity, _module) = pip_content_fixture();
        let venv = installed.parent().unwrap().parent().unwrap();
        std::fs::write(venv.join("requirements.txt"), "evil==1.0 --hash=sha256:00").unwrap();

        let error = verify_pip_install(&installed, &identity).unwrap_err();

        assert!(error.contains("requirements"), "{error}");
    }

    #[test]
    fn pip_verifier_rejects_tampered_installed_metadata() {
        let (_tmp, installed, identity, _module) = pip_content_fixture();
        let venv = installed.parent().unwrap().parent().unwrap();
        let site = pip_site_packages(venv).unwrap();
        let dist = site.join(format!("python_lsp_server-{}.dist-info", identity.version));
        std::fs::write(
            dist.join("METADATA"),
            format!("Name: {}\nVersion: 0.0.0-tampered\n", identity.package),
        )
        .unwrap();

        let error = verify_pip_install(&installed, &identity).unwrap_err();

        assert!(error.contains("套件"), "{error}");
    }

    #[test]
    fn npm_verify_rejects_tampered_wrapper_and_module() {
        let (_tmp, installed, identity, module) = npm_content_fixture();
        std::fs::write(&installed, "#!/bin/sh\nexec evil\n").unwrap();
        let error = verify_npm_install(&installed, &identity).unwrap_err();
        assert!(error.contains("launcher"), "{error}");

        write_npm_launcher(&installed, &identity).unwrap();
        std::fs::write(&module, "console.log('tampered');\n").unwrap();
        let error = verify_npm_install(&installed, &identity).unwrap_err();
        assert!(error.contains("package/module tree"), "{error}");
    }

    #[test]
    fn pip_verify_rejects_tampered_console_script_and_python_module() {
        let (_tmp, installed, identity, module) = pip_content_fixture();
        std::fs::write(&installed, "#!/bin/sh\nexec evil\n").unwrap();
        let error = verify_pip_install(&installed, &identity).unwrap_err();
        assert!(error.contains("launcher"), "{error}");

        write_pip_launcher(&installed, &identity).unwrap();
        std::fs::write(&module, "raise RuntimeError('tampered')\n").unwrap();
        let error = verify_pip_install(&installed, &identity).unwrap_err();
        assert!(error.contains("package/module tree"), "{error}");
    }

    #[test]
    fn allow_scripts_without_rationale_is_rejected() {
        let err = parse_catalog(
            r#"{
              "catalogVersion": "x",
              "servers": [{
                "language": "typescript",
                "serverId": "vtsls",
                "kind": "npm",
                "version": "1.0.0",
                "allowScripts": true,
                "platforms": { "*": { "status": "supported" } }
              }]
            }"#,
        )
        .unwrap_err();
        assert!(err.contains("rationale"), "{err}");
    }
}
