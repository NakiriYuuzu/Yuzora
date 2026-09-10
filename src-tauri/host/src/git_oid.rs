// Typed full commit OID + central revision resolver.
//
// Renderer-supplied revisions must pass `resolve_commit_oid` before any later
// Git invocation. The returned `GitOid` is a complete hex object name for the
// repository object format (SHA-1 = 40, SHA-256 = 64). Refs, prefixes, options,
// and revision expressions are never stored in this type.

use crate::git_service::run_git;
use std::path::Path;
use std::time::Duration;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitOid(String);

impl GitOid {
    pub const SHA1_HEX_LEN: usize = 40;
    pub const SHA256_HEX_LEN: usize = 64;

    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if !is_full_oid(value) {
            return Err("git rejected a non-OID object name".to_string());
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn short(&self) -> &str {
        let n = if self.0.len() == Self::SHA256_HEX_LEN {
            12
        } else {
            7
        };
        &self.0[..n]
    }
}

pub fn is_full_oid(value: &str) -> bool {
    let len = value.len();
    (len == GitOid::SHA1_HEX_LEN || len == GitOid::SHA256_HEX_LEN)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Hex-only OID prefix for the hash query arm (never branch names / HEAD / ~).
pub fn is_hex_oid_prefix(value: &str, min_len: usize) -> bool {
    let len = value.len();
    (min_len..=GitOid::SHA256_HEX_LEN).contains(&len)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn looks_like_git_option(value: &str) -> bool {
    value.starts_with('-')
}

fn no_replace_env() -> Vec<(String, String)> {
    vec![("GIT_NO_REPLACE_OBJECTS".to_string(), "1".to_string())]
}

/// Resolve a revision expression to a typed full commit OID.
///
/// Option-like values are rejected before any Git process is spawned so
/// payloads such as `--output=<path>` cannot become argv flags.
pub fn resolve_commit_oid(root: &Path, spec: &str) -> Result<GitOid, String> {
    resolve_commit_oid_optional(root, spec)?.ok_or_else(|| {
        if looks_like_git_option(spec) {
            "git rejected an option-like revision".to_string()
        } else {
            format!("git could not resolve revision '{spec}'")
        }
    })
}

pub fn resolve_commit_oid_optional(root: &Path, spec: &str) -> Result<Option<GitOid>, String> {
    if spec.is_empty() || spec.contains('\0') {
        return Err("git rejected an empty or NUL-containing revision".to_string());
    }
    if looks_like_git_option(spec) {
        return Err("git rejected an option-like revision".to_string());
    }
    let peeled = format!("{spec}^{{commit}}");
    let out = run_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &peeled,
        ],
        RESOLVE_TIMEOUT,
        &no_replace_env(),
    )?;
    if out.code != 0 {
        return Ok(None);
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        return Ok(None);
    }
    Ok(Some(GitOid::parse(&hash)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_sha1_and_sha256_hex() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(GitOid::parse(sha1).unwrap().as_str(), sha1);
        assert_eq!(GitOid::parse(sha256).unwrap().as_str(), sha256);
        assert_eq!(
            GitOid::parse(&sha1.to_ascii_uppercase()).unwrap().as_str(),
            sha1
        );
    }

    #[test]
    fn parse_rejects_prefix_ref_and_option() {
        assert!(GitOid::parse("abc1234").is_err());
        assert!(GitOid::parse("HEAD").is_err());
        assert!(GitOid::parse("main").is_err());
        assert!(GitOid::parse("--output=/tmp/x").is_err());
        assert!(GitOid::parse("").is_err());
    }

    #[test]
    fn option_like_revision_is_detected_before_spawn() {
        assert!(looks_like_git_option("--output=/tmp/pwned"));
        assert!(looks_like_git_option("-s"));
        assert!(!looks_like_git_option("HEAD"));
        assert!(!looks_like_git_option("refs/tags/--help"));
    }

    #[test]
    fn resolve_commit_oid_rejects_option_like_before_git_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = tmp.path().join("pwned");
        let inject = format!("--output={}", sink.display());
        let err = resolve_commit_oid(tmp.path(), &inject).unwrap_err();
        assert!(
            err.contains("option-like"),
            "expected option rejection, got {err}"
        );
        assert!(
            !sink.exists(),
            "option-like revision must not spawn git show --output"
        );
    }
}
