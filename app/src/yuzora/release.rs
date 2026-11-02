//! L7: how Yuzora is packaged and kept up to date (docs/beta2/release.md).
//!
//! - Names. Cargo's program stays `herdr-app`: upstream's tests and scripts
//!   use it. Packages call it Yuzora: `Yuzora.app/Contents/MacOS/Yuzora`,
//!   `yuzora\yuzora.exe`, `yuzora/yuzora`, and the CLI is `yuzora`
//!   (scripts/yuzora/package). Nothing of Yuzora's is ever named `herdr-app`
//!   on PATH, so the official Herdr app's CLI stays its own.
//! - Updates. [`update`] finds a newer release on Yuzora's GitHub releases,
//!   on the stable or the beta channel, from a manifest signed with the
//!   release key; upstream's src/update.rs then downloads, checks, stages and
//!   swaps it the way it does for Herdr.
//! - [`install_cli_link`]: `~/.local/bin/yuzora` for an installed Yuzora.app.

pub(crate) mod channel;
#[cfg(any(target_os = "macos", all(test, unix)))]
mod cli_link;
pub(crate) mod manifest;
pub(crate) mod update;

use std::path::{Path, PathBuf};

/// The folder or bundle inside an update archive.
pub const PACKED: &str = if cfg!(target_os = "macos") { "Yuzora.app" } else { "yuzora" };

/// Beside the program in a Linux or Windows release folder (cargo's target/
/// has none): how the app knows it's an installed release.
pub const MARKER: &str = "yuzora.png";

/// The program in an installed or unpacked app (`app`: the bundle or folder).
pub fn exe_in(app: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        app.join("Contents/MacOS/Yuzora")
    } else {
        app.join(format!("yuzora{}", std::env::consts::EXE_SUFFIX))
    }
}

/// This machine in the manifest's `targets` (`macos-aarch64`, `windows-x86_64`, …).
pub fn target() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// Semver precedence of two pre-release parts (`beta.10` after `beta.9`,
/// numbers before words). Upstream compared them as plain strings.
pub fn prerelease_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.split('.'), b.split('.'));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// `~/.local/bin/yuzora` for an installed Yuzora.app: a DMG has no installer
/// to put the CLI on PATH. Never touches `herdr-app` (the official Herdr's).
#[cfg(target_os = "macos")]
pub fn install_cli_link() {
    if let Err(e) = std::thread::Builder::new().name("yuzora-cli-link".into()).spawn(|| {
        let result = std::env::current_exe().and_then(|exe| cli_link::ensure(&crate::mods::home(), &exe));
        if let Err(e) = result {
            tracing::warn!("cannot install the yuzora CLI link: {e}");
        }
    }) {
        tracing::warn!("cannot start the yuzora CLI setup: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_are_named_yuzora() {
        let app = Path::new("/x").join(PACKED);
        let exe = exe_in(&app);
        if cfg!(target_os = "macos") {
            assert_eq!(exe, Path::new("/x/Yuzora.app/Contents/MacOS/Yuzora"));
        } else {
            assert_eq!(exe.file_stem().unwrap(), "yuzora");
        }
        assert!(!exe.to_string_lossy().contains("herdr"));
        assert!(["macos-aarch64", "macos-x86_64", "linux-x86_64", "linux-aarch64", "windows-x86_64", "windows-aarch64"].contains(&target().as_str()));
    }

    #[test]
    fn prereleases_compare_like_semver() {
        use std::cmp::Ordering::*;
        assert_eq!(prerelease_cmp("beta.10", "beta.9"), Greater);
        assert_eq!(prerelease_cmp("beta.2", "beta.2"), Equal);
        assert_eq!(prerelease_cmp("beta", "beta.1"), Less);
        assert_eq!(prerelease_cmp("alpha.9", "beta.1"), Less);
        assert_eq!(prerelease_cmp("1", "beta"), Less);
        // The app's own comparison (src/update.rs) goes through it.
        assert!(crate::update::newer("0.1.0-beta.10", "0.1.0-beta.9"));
        assert!(!crate::update::newer("0.1.0-beta.9", "0.1.0-beta.10"));
        assert!(crate::update::newer("0.1.0", "0.1.0-beta.10"));
        assert!(crate::update::newer("0.1.0-beta.2", "0.1.0-beta.1"));
        assert!(crate::update::newer("0.1.0-beta.2", "0.0.19"));
    }
}
