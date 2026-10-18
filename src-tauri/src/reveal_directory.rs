use std::io::Read;
use std::path::{Path, PathBuf};
use tauri_plugin_opener::OpenerExt;

use crate::path_capability::WorkspacePathRegistry;

fn workspace_directory(
    workspaces: &WorkspacePathRegistry,
    workspace_id: &str,
    path: &str,
) -> Result<PathBuf, String> {
    let root = workspaces
        .canonical_root(workspace_id)
        .map_err(String::from)?;
    let directory = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !directory.starts_with(Path::new(&root)) || !directory.is_dir() {
        return Err("Only directories in the current workspace can be opened".into());
    }
    // Canonicalization may race a workspace switch. Never accept a revoked token.
    workspaces
        .canonical_root(workspace_id)
        .map_err(String::from)?;
    Ok(directory)
}

/// Whether macOS treats `directory` as a package (an `.app`, an installer `.pkg`, a
/// `.prefPane`…). Opening a package hands it to its application, so an `.app` from
/// the workspace would be launched instead of shown.
#[cfg(target_os = "macos")]
fn is_package(directory: &str) -> bool {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    NSWorkspace::sharedWorkspace().isFilePackageAtPath(&NSString::from_str(directory))
}

#[cfg(not(target_os = "macos"))]
fn is_package(_directory: &str) -> bool {
    false
}

#[derive(Debug, PartialEq, Eq)]
enum DirectoryAction {
    Open,
    /// Select the package in its parent folder instead of handing it to its application.
    Reveal,
}

fn directory_action(directory: &str) -> DirectoryAction {
    if is_package(directory) {
        DirectoryAction::Reveal
    } else {
        DirectoryAction::Open
    }
}

/// Open a directory, never an arbitrary file, through the existing workspace grant.
/// A package is revealed in its parent folder instead, so it is never launched.
/// The frontend's generic opener allowlist remains unchanged.
#[tauri::command(async)]
pub fn open_workspace_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    workspace_id: String,
    path: String,
) -> Result<(), String> {
    let directory = workspace_directory(&state.0, &workspace_id, &path)?;
    let directory = directory
        .to_str()
        .ok_or("Directory path is not valid UTF-8")?;
    match directory_action(directory) {
        DirectoryAction::Reveal => app.opener().reveal_item_in_dir(directory),
        DirectoryAction::Open => app.opener().open_path(directory, None::<&str>),
    }
    .map_err(|error| error.to_string())
}

/// Extensions the OS shell launches or runs as a script instead of merely showing.
/// Best effort, not a sandbox: it keeps a file from an untrusted workspace from being
/// launched by the "open externally" button. Revealing such a file stays possible.
const LAUNCHABLE_EXTENSIONS: &[&str] = &[
    "app", "appimage", "bash", "bat", "cmd", "com", "command", "cpl", "csh", "desktop", "exe",
    "fish", "hta", "jar", "js", "jse", "ksh", "lnk", "mpkg", "msc", "msi", "msp", "pif", "pkg",
    "ps1", "psm1", "reg", "run", "scpt", "scr", "sh", "terminal", "url", "vb", "vbe", "vbs",
    "workflow", "ws", "wsf", "wsh", "zsh",
];

fn is_launchable(path: &Path) -> std::io::Result<bool> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            LAUNCHABLE_EXTENSIONS
                .iter()
                .any(|launchable| extension.eq_ignore_ascii_case(launchable))
        })
    {
        return Ok(true);
    }
    // Extensionless executables and scripts are recognised by their header.
    let mut head = Vec::with_capacity(8);
    std::fs::File::open(path)?.take(8).read_to_end(&mut head)?;
    Ok(head.starts_with(b"#!")
        || head.starts_with(b"MZ")
        || head.starts_with(b"\x7fELF")
        || matches!(
            head[..],
            [0xFE, 0xED, 0xFA, 0xCE | 0xCF, ..]
                | [0xCE | 0xCF, 0xFA, 0xED, 0xFE, ..]
                | [0xCA, 0xFE, 0xBA, 0xBF, ..]
        )
        || match head[..] {
            // Universal Mach-O shares 0xCAFEBABE with Java class files; its
            // architecture count stays below any class-file major version (45+).
            [0xCA, 0xFE, 0xBA, 0xBE, a, b, c, d] => u32::from_be_bytes([a, b, c, d]) < 45,
            [0xCA, 0xFE, 0xBA, 0xBE, ..] => true,
            _ => false,
        })
}

fn workspace_file(
    workspaces: &WorkspacePathRegistry,
    workspace_id: &str,
    path: &str,
) -> Result<PathBuf, String> {
    let root = workspaces
        .canonical_root(workspace_id)
        .map_err(String::from)?;
    let file = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !file.starts_with(Path::new(&root)) || !file.is_file() {
        return Err("Only files in the current workspace can be opened".into());
    }
    if is_launchable(&file).map_err(|error| error.to_string())? {
        return Err(
            "Programs and scripts are not launched from here; reveal the file in the file manager instead"
                .into(),
        );
    }
    // Canonicalization may race a workspace switch. Never accept a revoked token.
    workspaces
        .canonical_root(workspace_id)
        .map_err(String::from)?;
    Ok(file)
}

/// Open a file in the system's default program through the existing workspace grant.
/// The frontend's generic opener allowlist remains unchanged.
#[tauri::command(async)]
pub fn open_workspace_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::path_capability::WorkspacePathState>,
    workspace_id: String,
    path: String,
) -> Result<(), String> {
    let file = workspace_file(&state.0, &workspace_id, &path)?;
    let file = file.to_str().ok_or("File path is not valid UTF-8")?;
    app.opener()
        .open_path(file, None::<&str>)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_current_workspace_directories_and_rejects_files_outside_and_revoked_tokens() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let child = root.path().join("中文 project");
        std::fs::create_dir(&child).unwrap();
        let file = root.path().join("script.sh");
        std::fs::write(&file, "echo test").unwrap();
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(root.path()).unwrap();
        assert!(workspace_directory(&registry, &id, root.path().to_str().unwrap()).is_ok());
        assert!(workspace_directory(&registry, &id, child.to_str().unwrap()).is_ok());
        assert!(workspace_directory(&registry, &id, file.to_str().unwrap()).is_err());
        assert!(workspace_directory(&registry, &id, outside.path().to_str().unwrap()).is_err());
        registry.activate(outside.path()).unwrap();
        assert!(workspace_directory(&registry, &id, child.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_directory_symlink_escaping_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(root.path()).unwrap();
        assert!(workspace_directory(&registry, &id, link.to_str().unwrap()).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundles_are_revealed_and_plain_folders_are_opened() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Tools.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let folder = root.path().join("tools");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(
            directory_action(bundle.to_str().unwrap()),
            DirectoryAction::Reveal
        );
        assert_eq!(
            directory_action(folder.to_str().unwrap()),
            DirectoryAction::Open
        );
    }

    fn workspace_with(
        files: &[(&str, &[u8])],
    ) -> (tempfile::TempDir, WorkspacePathRegistry, String) {
        let root = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(root.path().join(name), body).unwrap();
        }
        let registry = WorkspacePathRegistry::new();
        let id = registry.activate(root.path()).unwrap();
        (root, registry, id)
    }

    fn open_check(
        root: &tempfile::TempDir,
        registry: &WorkspacePathRegistry,
        id: &str,
        name: &str,
    ) -> Result<PathBuf, String> {
        workspace_file(registry, id, root.path().join(name).to_str().unwrap())
    }

    #[test]
    fn opens_regular_files_inside_the_current_workspace() {
        let (root, registry, id) = workspace_with(&[
            ("中文 doc.pdf", b"%PDF-1.7"),
            ("archive.zip", b"PK\x03\x04"),
            ("blob.bin", &[0, 1, 2, 3]),
            ("noext", b"plain"),
        ]);
        for name in ["中文 doc.pdf", "archive.zip", "blob.bin", "noext"] {
            let file = open_check(&root, &registry, &id, name).unwrap();
            assert!(file.is_file(), "{name}");
        }
    }

    #[test]
    fn rejects_directories_missing_files_outside_files_and_revoked_tokens() {
        let (root, registry, id) = workspace_with(&[("a.pdf", b"%PDF")]);
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("b.pdf");
        std::fs::write(&elsewhere, b"%PDF").unwrap();
        std::fs::create_dir(root.path().join("dir")).unwrap();
        assert!(open_check(&root, &registry, &id, "dir").is_err());
        assert!(open_check(&root, &registry, &id, "missing.pdf").is_err());
        assert!(workspace_file(&registry, &id, elsewhere.to_str().unwrap()).is_err());
        assert!(workspace_file(
            &registry,
            "ws-forged",
            root.path().join("a.pdf").to_str().unwrap()
        )
        .is_err());
        registry.activate(outside.path()).unwrap();
        assert!(open_check(&root, &registry, &id, "a.pdf").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_file_symlink_escaping_the_workspace() {
        let (root, registry, id) = workspace_with(&[]);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.pdf");
        std::fs::write(&target, b"%PDF").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("link.pdf")).unwrap();
        assert!(open_check(&root, &registry, &id, "link.pdf").is_err());
    }

    #[test]
    fn refuses_programs_and_scripts_by_extension_or_header() {
        let (root, registry, id) = workspace_with(&[
            ("setup.EXE", b"x"),
            ("run.sh", b"x"),
            ("macro.Ps1", b"x"),
            ("tool.jar", b"x"),
            ("no-extension-script", b"#!/bin/sh\necho hi\n"),
            ("linux-binary", b"\x7fELF\x02\x01"),
            ("windows-binary", b"MZ\x90\x00"),
            ("mac-binary", &[0xCF, 0xFA, 0xED, 0xFE, 0x07]),
            ("mac-universal", &[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 2, 0x01]),
            ("mac-universal-64", &[0xCA, 0xFE, 0xBA, 0xBF, 0, 0, 0, 2]),
            ("truncated-universal", &[0xCA, 0xFE, 0xBA, 0xBE, 0]),
        ]);
        for name in [
            "setup.EXE",
            "run.sh",
            "macro.Ps1",
            "tool.jar",
            "no-extension-script",
            "linux-binary",
            "windows-binary",
            "mac-binary",
            "mac-universal",
            "mac-universal-64",
            "truncated-universal",
        ] {
            let error = open_check(&root, &registry, &id, name).unwrap_err();
            assert!(error.contains("reveal"), "{name}: {error}");
        }
    }

    #[test]
    fn java_class_files_are_not_mistaken_for_universal_binaries() {
        // Same 0xCAFEBABE magic; the next u32 is minor/major version (Java 21 = 65).
        let (root, registry, id) =
            workspace_with(&[("Main.class", &[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 65])]);
        assert!(open_check(&root, &registry, &id, "Main.class").is_ok());
    }

    #[test]
    fn short_and_empty_files_are_still_openable() {
        let (root, registry, id) = workspace_with(&[("empty.dat", b""), ("m.dat", b"M")]);
        assert!(open_check(&root, &registry, &id, "empty.dat").is_ok());
        assert!(open_check(&root, &registry, &id, "m.dat").is_ok());
    }
}
