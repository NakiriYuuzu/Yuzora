//! Where the bundled `herdr-core` binary is.
//!
//! Release: the Tauri resource dir, `herdr-core/<target>/herdr-core(.exe)`. Debug: the source
//! tree's `src-tauri/resources/herdr-core/<target>/` (what `bun run herdr-core:prepare`
//! fills), then the resource dir. The file is only looked for, never run, here.
//!
//! P4 (Windows): the binary is verified and copied into a versioned directory under the app's
//! local data dir and run from there. That replaces [`bundled_binary`] (the one function
//! `Deps::production` plugs in); nothing else needs to know where the file comes from.
use std::path::{Path, PathBuf};

use super::service::Paths;

/// The resource directory name of this platform, as `herdr-core-runtime.json` names it.
pub fn target_dir() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    format!("{os}-{arch}")
}

pub fn executable_name(windows: bool) -> &'static str {
    if windows {
        "herdr-core.exe"
    } else {
        "herdr-core"
    }
}

/// `<root>/herdr-core/<target>/herdr-core(.exe)`.
pub fn path_under(root: &Path, target: &str, windows: bool) -> PathBuf {
    root.join("herdr-core")
        .join(target)
        .join(executable_name(windows))
}

/// The source tree's resources, in debug builds only (the path is not baked into release
/// builds).
#[cfg(debug_assertions)]
pub fn dev_resources_root() -> Option<PathBuf> {
    Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

#[cfg(not(debug_assertions))]
pub fn dev_resources_root() -> Option<PathBuf> {
    None
}

/// The first of `roots` that holds the binary; with none, where the first would be (so the
/// failure names a path); with no roots at all, an empty path.
pub fn resolve(roots: &[PathBuf], target: &str, windows: bool) -> PathBuf {
    let candidates: Vec<PathBuf> = roots
        .iter()
        .map(|root| path_under(root, target, windows))
        .collect();
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .or_else(|| candidates.first())
        .cloned()
        .unwrap_or_default()
}

/// The bundled binary for this build and platform.
pub fn bundled_binary(paths: &Paths) -> PathBuf {
    let roots: Vec<PathBuf> = dev_resources_root()
        .into_iter()
        .chain(paths.resource_dir.clone())
        .collect();
    resolve(&roots, &target_dir(), cfg!(windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn herdr_core_binary_path_layout_matches_the_resource_directory() {
        let root = Path::new("/r");
        assert_eq!(
            path_under(root, "macos-aarch64", false),
            Path::new("/r/herdr-core/macos-aarch64/herdr-core")
        );
        assert_eq!(
            path_under(root, "windows-x86_64", true),
            Path::new("/r/herdr-core/windows-x86_64/herdr-core.exe")
        );
        let target = target_dir();
        assert!(
            [
                "macos-aarch64",
                "macos-x86_64",
                "windows-x86_64",
                "windows-aarch64",
                "linux-x86_64",
                "linux-aarch64"
            ]
            .contains(&target.as_str()),
            "{target}"
        );
    }

    #[test]
    fn herdr_core_binary_resolution_prefers_the_first_root_that_has_the_file() {
        let dev = tempfile::tempdir().unwrap();
        let installed = tempfile::tempdir().unwrap();
        let roots = [dev.path().to_path_buf(), installed.path().to_path_buf()];
        let in_dev = path_under(dev.path(), "t", false);
        let in_installed = path_under(installed.path(), "t", false);

        // Nothing anywhere: the first root's path, so the failure says where it looked.
        assert_eq!(resolve(&roots, "t", false), in_dev);
        touch(&in_installed);
        assert_eq!(resolve(&roots, "t", false), in_installed);
        touch(&in_dev);
        assert_eq!(resolve(&roots, "t", false), in_dev);
        assert_eq!(resolve(&[], "t", false), PathBuf::new());
    }

    #[test]
    fn herdr_core_binary_dev_root_exists_only_in_debug_builds() {
        let root = dev_resources_root();
        if cfg!(debug_assertions) {
            let root = root.expect("debug builds read the source tree's resources");
            assert!(root.ends_with("resources"), "{}", root.display());
        } else {
            assert_eq!(root, None);
        }
    }

    #[test]
    fn herdr_core_bundled_binary_uses_the_resource_dir_when_no_dev_copy_exists() {
        let installed = tempfile::tempdir().unwrap();
        let expected = path_under(installed.path(), &target_dir(), cfg!(windows));
        touch(&expected);
        let paths = Paths {
            data_dir: PathBuf::from("/data"),
            resource_dir: Some(installed.path().to_path_buf()),
        };
        let found = bundled_binary(&paths);
        // A debug build prefers the source tree's copy when `herdr-core:prepare` has run.
        let dev = dev_resources_root().map(|root| path_under(&root, &target_dir(), cfg!(windows)));
        if dev.as_ref().is_some_and(|dev| dev.is_file()) {
            assert_eq!(Some(found), dev);
        } else {
            assert_eq!(found, expected);
        }
    }
}
