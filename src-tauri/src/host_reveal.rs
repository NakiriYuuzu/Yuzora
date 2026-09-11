//! WSL Explorer routing stays bound to a connected host generation.
use crate::host_service::{HostState, HostTarget};
use yuzora_host::protocol::ConnectionOwner;

#[derive(Debug, PartialEq, Eq)]
struct ExplorerTarget {
    folder: String,
    selected: Option<String>,
}

fn explorer_target(distro: &str, path: &str, is_directory: bool) -> Result<ExplorerTarget, String> {
    if distro.is_empty()
        || matches!(distro, "." | "..")
        || distro.contains([
            '\\', '/', ':', '"', '<', '>', '|', '?', '*', '\0', '\r', '\n',
        ])
        || !path.starts_with('/')
        || path.contains(['\\', ':', '"', '<', '>', '|', '?', '*', '\0', '\r', '\n'])
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err("invalid-wsl-explorer-path".into());
    }
    let path = path.trim_end_matches('/');
    let unc = format!(r"\\wsl.localhost\{distro}{}", path.replace('/', r"\"));
    if is_directory {
        return Ok(ExplorerTarget {
            folder: unc,
            selected: None,
        });
    }
    let (parent, name) = path.rsplit_once('/').ok_or("invalid-wsl-explorer-file")?;
    if name.is_empty() {
        return Err("invalid-wsl-explorer-file".into());
    }
    Ok(ExplorerTarget {
        folder: format!(r"\\wsl.localhost\{distro}{}", parent.replace('/', r"\")),
        selected: Some(unc),
    })
}

#[cfg(any(windows, test))]
fn legacy_wsl_folder(path: &str) -> Option<String> {
    path.strip_prefix(r"\\wsl.localhost\")
        .map(|rest| format!(r"\\wsl$\{rest}"))
}

#[cfg(any(windows, test))]
fn reveal_with_fallback(
    select: impl FnOnce() -> Result<(), String>,
    explore_folder: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    select().or_else(|selection_error| {
        explore_folder().map_err(|fallback_error| {
            format!("Explorer selection failed: {selection_error}; opening folder failed: {fallback_error}")
        })
    })
}

#[tauri::command]
pub async fn host_reveal_in_explorer(
    state: tauri::State<'_, HostState>,
    owner: ConnectionOwner,
    path: String,
    is_directory: bool,
) -> Result<(), String> {
    let connection = state.0.connection(&owner)?;
    let HostTarget::Wsl { distro } = &connection.target else {
        return Err("explorer-requires-connected-wsl-host".into());
    };
    let target = explorer_target(distro, &path, is_directory)?;
    crate::host_wsl::verify_identity(&owner.host_id, distro).await?;
    state.0.connection(&owner)?;
    #[cfg(windows)]
    {
        let manager = state.0.clone();
        tokio::task::spawn_blocking(move || {
            manager.connection(&owner)?;
            windows::reveal(&target, || manager.connection(&owner).map(|_| ()))
        })
        .await
        .map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err("explorer-requires-windows".into())
    }
}

#[cfg(windows)]
mod windows {
    use super::{legacy_wsl_folder, reveal_with_fallback, ExplorerTarget};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Com::{CoInitialize, CoTaskMemFree, CoUninitialize};
    use windows_sys::Win32::System::SystemServices::SFGAO_FOLDER;
    use windows_sys::Win32::UI::Shell::{
        Common::ITEMIDLIST, ILFindLastID, SHOpenFolderAndSelectItems, SHParseDisplayName,
        ShellExecuteExW, SEE_MASK_CLASSNAME, SHELLEXECUTEINFOW,
    };

    struct Pidl(*mut ITEMIDLIST);
    impl Pidl {
        fn parse(path: &str) -> Result<Self, String> {
            Self::parse_with_attributes(path, 0).map(|(item, _)| item)
        }

        fn parse_folder(path: &str) -> Result<Self, String> {
            let (item, attributes) = Self::parse_with_attributes(path, SFGAO_FOLDER)?;
            let item = item?;
            if attributes & SFGAO_FOLDER == 0 {
                return Err("explorer-target-is-not-directory".into());
            }
            Ok(item)
        }

        fn parse_with_attributes(
            path: &str,
            requested_attributes: u32,
        ) -> Result<(Self, u32), String> {
            let wide = wide(path);
            let mut item = null_mut();
            let mut attributes = 0;
            let result = unsafe {
                SHParseDisplayName(
                    wide.as_ptr(),
                    null_mut(),
                    &mut item,
                    requested_attributes,
                    &mut attributes,
                )
            };
            if result < 0 || item.is_null() {
                if !item.is_null() {
                    unsafe { CoTaskMemFree(item.cast()) }
                }
                return Err(format!("parse Shell path HRESULT {result:#x}"));
            }
            Ok((Self(item), attributes))
        }
    }
    impl Drop for Pidl {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(self.0.cast()) }
        }
    }
    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() }
            }
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    pub(super) fn reveal(
        target: &ExplorerTarget,
        current: impl Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        let initialized = unsafe { CoInitialize(null()) };
        // RPC_E_CHANGED_MODE means this pooled thread already owns a COM apartment.
        if initialized < 0 && initialized != 0x80010106_u32 as i32 {
            return Err(format!("initialize Shell COM HRESULT {initialized:#x}"));
        }
        let _com = Com(initialized >= 0);
        reveal_with_fallback(
            || {
                // WSL UNC paths are a Shell namespace. Do not preflight them
                // with std::fs::metadata: that API can reject a path which
                // Explorer can still resolve or start. SHParseDisplayName is
                // both the namespace-aware lookup and the folder safety check.
                let folder = Pidl::parse_folder(&target.folder)?;
                let selected = target.selected.as_deref().map(Pidl::parse).transpose()?;
                current()?;
                let result = if let Some(selected) = selected {
                    let child = unsafe { ILFindLastID(selected.0) } as *const ITEMIDLIST;
                    unsafe { SHOpenFolderAndSelectItems(folder.0, 1, &child, 0) }
                } else {
                    unsafe { SHOpenFolderAndSelectItems(folder.0, 0, null(), 0) }
                };
                if result < 0 {
                    return Err(format!("open Shell folder HRESULT {result:#x}"));
                }
                Ok(())
            },
            || {
                // A failed selection must still open Explorer. Use an explicit
                // folder verb, never execute the selected file or a command line.
                let mut last_error = None;
                // Keep the alternate WSL namespace as a compatibility fallback
                // for Windows builds where .localhost is not registered even
                // though the legacy provider is.
                let alternate_folder = legacy_wsl_folder(&target.folder);
                for folder_path in
                    std::iter::once(target.folder.as_str()).chain(alternate_folder.as_deref())
                {
                    let verb = wide("explore");
                    let folder = wide(folder_path);
                    let class = wide("folder");
                    let mut info = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        fMask: SEE_MASK_CLASSNAME,
                        lpVerb: verb.as_ptr(),
                        lpFile: folder.as_ptr(),
                        lpClass: class.as_ptr(),
                        nShow: 1,
                        ..Default::default()
                    };
                    current()?;
                    if unsafe { ShellExecuteExW(&mut info) } != 0 {
                        return Ok(());
                    }
                    last_error = Some(unsafe {
                        format!(
                            "explore folder Shell error {}",
                            windows_sys::Win32::Foundation::GetLastError()
                        )
                    });
                }
                Err(last_error.unwrap_or_else(|| "explore folder Shell error".into()))
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_unicode_spaces_and_commas_without_shell_arguments() {
        assert_eq!(
            explorer_target("Ubuntu-24.04", "/home/中文, project/a.ts", false).unwrap(),
            ExplorerTarget {
                folder: r"\\wsl.localhost\Ubuntu-24.04\home\中文, project".into(),
                selected: Some(r"\\wsl.localhost\Ubuntu-24.04\home\中文, project\a.ts".into()),
            }
        );
        assert_eq!(
            explorer_target("Ubuntu", "/home/project/", true).unwrap(),
            ExplorerTarget {
                folder: r"\\wsl.localhost\Ubuntu\home\project".into(),
                selected: None,
            }
        );
        assert_eq!(
            explorer_target("Ubuntu", "/", true).unwrap().folder,
            r"\\wsl.localhost\Ubuntu"
        );
        assert_eq!(
            legacy_wsl_folder(r"\\wsl.localhost\Ubuntu\home\project"),
            Some(r"\\wsl$\Ubuntu\home\project".into())
        );
        assert_eq!(legacy_wsl_folder(r"C:\project"), None);
    }
    #[test]
    fn rejects_other_namespaces_traversal_and_windows_shell_characters() {
        for path in [
            "relative",
            r"C:\file",
            "//../Other/file",
            "/home/../file",
            "/home/./file",
            "/file\0",
            "/file\"",
            r"/home\other/file",
        ] {
            assert!(explorer_target("Ubuntu", path, false).is_err(), "{path}");
        }
        assert!(explorer_target(r"Ubuntu\Other", "/home", true).is_err());
        assert!(explorer_target("Ubuntu", "/", false).is_err());
    }
    #[test]
    fn every_shell_preparation_error_falls_back_and_fallback_failure_is_reported() {
        use std::cell::Cell;
        let called = Cell::new(false);
        assert!(reveal_with_fallback(
            || Err("parse Shell path HRESULT 0x80070035".into()),
            || {
                called.set(true);
                Ok(())
            }
        )
        .is_ok());
        assert!(called.get());
        called.set(false);
        assert!(reveal_with_fallback(
            || Ok(()),
            || {
                called.set(true);
                Ok(())
            }
        )
        .is_ok());
        assert!(!called.get());
        let failure = reveal_with_fallback(
            || Err("parse failed".into()),
            || Err("folder unavailable".into()),
        )
        .unwrap_err();
        assert!(failure.contains("parse failed") && failure.contains("folder unavailable"));
    }
}
