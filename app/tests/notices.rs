use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

fn lock_inputs(lock: &str, workspace_names: &BTreeSet<&str>) -> String {
    let mut packages = Vec::new();
    for package in lock.split("[[package]]").skip(1) {
        let field = |key: &str| {
            package.lines().find_map(|line| {
                line.strip_prefix(key)?
                    .strip_prefix(" = \"")?
                    .strip_suffix('"')
            })
        };
        let name = field("name").unwrap();
        let source = field("source").unwrap_or("");
        if source.is_empty() && workspace_names.contains(name) {
            continue;
        }
        packages.push(format!(
            "{name}\t{}\t{source}\t{}",
            field("version").unwrap(),
            field("checksum").unwrap_or("")
        ));
    }
    packages.sort();
    packages.join("\n")
}

fn manifest_inputs(manifest: &str) -> String {
    let mut in_package = false;
    manifest
        .split('\n')
        .map(|line| {
            if line.starts_with('[') {
                in_package = line == "[package]";
            }
            if in_package && line.starts_with("version = ") {
                "version = \"<workspace version>\""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lock_fingerprint_tracks_third_party_packages_only() {
    let workspace = BTreeSet::from(["herdr-app"]);
    let lock = r#"
[[package]]
name = "herdr-app"
version = "0.1.7"
[[package]]
name = "registry-dep"
version = "1.2.3"
source = "registry+https://example.com/index"
checksum = "abc"
[[package]]
name = "vendored-dep"
version = "2.3.4"
"#;
    let fingerprint = lock_inputs(lock, &workspace);
    assert_eq!(
        fingerprint,
        lock_inputs(&lock.replace("0.1.7", "0.1.8"), &workspace)
    );
    for (old, new) in [
        ("1.2.3", "1.2.4"),
        ("2.3.4", "2.3.5"),
        ("registry-dep", "renamed-dep"),
        ("example.com", "other.com"),
        ("abc", "def"),
    ] {
        assert_ne!(
            fingerprint,
            lock_inputs(&lock.replace(old, new), &workspace)
        );
    }
    let reversed = lock
        .split("[[package]]")
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|package| format!("[[package]]{package}"))
        .collect::<String>();
    assert_eq!(fingerprint, lock_inputs(&reversed, &workspace));
}

#[test]
fn third_party_notices_are_current_and_complete() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| {
        fs::read_to_string(root.join(path))
            .unwrap()
            .replace("\r\n", "\n")
    };
    let metadata = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let workspace_names = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|package| {
            metadata["workspace_members"]
                .as_array()
                .unwrap()
                .contains(&package["id"])
        })
        .map(|package| package["name"].as_str().unwrap())
        .collect();
    let notices = read("THIRD-PARTY-NOTICES");
    let inventory: serde_json::Value =
        serde_json::from_str(&read("licenses/inventory.json")).unwrap();
    let mut inputs = BTreeSet::new();
    for path in inventory["inputs"].as_array().unwrap() {
        inputs.insert(path.as_str().unwrap());
    }
    assert!(inputs.contains("Cargo.lock"));
    for component in inventory["components"].as_array().unwrap() {
        for path in component["files"].as_array().unwrap() {
            let path = path.as_str().unwrap();
            inputs.insert(path);
            assert!(
                notices.contains(read(path).trim()),
                "{path} missing from THIRD-PARTY-NOTICES; run just notices"
            );
        }
    }
    for path in inputs {
        let content = read(path);
        let content = if path == "Cargo.lock" {
            lock_inputs(&content, &workspace_names)
        } else if path == "Cargo.toml" {
            manifest_inputs(&content)
        } else {
            content
        };
        let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        assert!(
            notices
                .lines()
                .any(|line| line == format!("Input-SHA256: {hash} {path}")),
            "{path} changed; review licenses and run just notices"
        );
    }
    assert!(notices.contains("Cargo dependencies"));
    assert!(notices.contains("quickjs-jit-sys"));
    assert!(notices.contains("gpui-pre"));
}
