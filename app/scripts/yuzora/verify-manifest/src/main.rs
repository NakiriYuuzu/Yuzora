//! `verify-manifest <manifest> <signature> <version> <downloads> [--targets a,b] [--extra-key <key>]`
//!
//! Exits 0 and prints one summary line only when `manifest` is signed by a key
//! in `manifest::TRUSTED_KEYS` (the app's own list) and passes every check the
//! installed app runs after the signature (`manifest::read`). `--targets`
//! also requires the manifest to list exactly those targets. `--extra-key`
//! trusts one more minisign public key line, for tests with a throwaway key;
//! the release workflow never passes it.
#![allow(dead_code)]

#[path = "../../../../src/yuzora/release/manifest.rs"]
mod manifest;

use std::collections::BTreeSet;

fn run(args: &[String]) -> Result<String, String> {
    let mut positional = Vec::new();
    let (mut targets, mut extra_key) = (None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--targets" => targets = Some(it.next().ok_or("--targets needs a value")?.clone()),
            "--extra-key" => extra_key = Some(it.next().ok_or("--extra-key needs a value")?.clone()),
            _ => positional.push(arg.clone()),
        }
    }
    let [manifest_path, signature_path, version, downloads] = positional.as_slice() else {
        return Err("usage: verify-manifest <manifest> <signature> <version> <downloads> [--targets a,b] [--extra-key <key>]".into());
    };
    let bytes = std::fs::read(manifest_path).map_err(|e| format!("{manifest_path}: {e}"))?;
    let signature = std::fs::read(signature_path).map_err(|e| format!("{signature_path}: {e}"))?;
    let mut keys: Vec<String> = manifest::TRUSTED_KEYS.iter().map(|k| (*k).to_owned()).collect();
    keys.extend(extra_key);
    let m = manifest::read(&bytes, &signature, &keys, version, downloads.trim_end_matches('/'))?;
    let listed: BTreeSet<&str> = m.targets.keys().map(String::as_str).collect();
    if let Some(wanted) = &targets {
        let wanted: BTreeSet<&str> = wanted.split(',').filter(|t| !t.is_empty()).collect();
        if wanted != listed {
            return Err(format!("{}: lists {listed:?}, expected exactly {wanted:?}", manifest::MANIFEST));
        }
    }
    Ok(format!(
        "verified {} {} ({}) targets={} notes={} localized_notes={}",
        m.name,
        m.version,
        m.channel,
        listed.into_iter().collect::<Vec<_>>().join(","),
        m.notes.is_some(),
        m.localized_notes.len()
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(line) => println!("{line}"),
        Err(e) => {
            eprintln!("verify-manifest: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../src/yuzora/release/fixtures");
    const DOWNLOADS: &str = "https://github.com/NakiriYuuzu/Yuzora/releases/download";

    fn args(extra: &[&str]) -> Vec<String> {
        let mut v = vec![format!("{DIR}/yuzora-update.json"), format!("{DIR}/yuzora-update.json.sig"), "0.1.0-beta.3".into(), DOWNLOADS.into()];
        v.extend(extra.iter().map(|s| (*s).to_owned()));
        v
    }

    fn test_key() -> String {
        std::fs::read_to_string(format!("{DIR}/test-key.pub")).unwrap().trim().to_owned()
    }

    #[test]
    fn only_the_apps_own_keys_count_unless_a_test_key_is_added() {
        let err = run(&args(&[])).unwrap_err();
        assert!(err.contains("different key"), "{err}");
        let ok = run(&args(&["--extra-key", &test_key()])).unwrap();
        assert!(ok.starts_with("verified yuzora 0.1.0-beta.3 (beta) targets="), "{ok}");
    }

    #[test]
    fn the_target_list_can_be_pinned() {
        let key = test_key();
        let line = run(&args(&["--extra-key", &key])).unwrap();
        let listed = line.split("targets=").nth(1).unwrap().split(' ').next().unwrap().to_owned();
        assert!(run(&args(&["--extra-key", &key, "--targets", &listed])).is_ok());
        let err = run(&args(&["--extra-key", &key, "--targets", "macos-aarch64"])).unwrap_err();
        assert!(err.contains("expected exactly"), "{err}");
    }

    #[test]
    fn a_manifest_for_another_release_or_place_is_refused() {
        let key = test_key();
        let mut other = args(&["--extra-key", &key]);
        other[2] = "0.1.0".into();
        assert!(run(&other).unwrap_err().contains("but the release is 0.1.0"));
        let mut elsewhere = args(&["--extra-key", &key]);
        elsewhere[3] = "https://example.com/releases/download".into();
        assert!(run(&elsewhere).unwrap_err().contains("not in this release"));
    }
}
