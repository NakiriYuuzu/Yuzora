//! Windows: hands an NSIS install over to the MSI that updated it (#116).
//!
//! Only the MSI ships since v0.0.18, and NSIS clients update through it. The
//! MSI reads the NSIS directory from `HKCU\Software\yuuzu\Yuzora` and installs
//! over it, which leaves two Add/Remove Programs entries for one set of files.
//! Elevated as another account it lands in Program Files instead, beside the
//! NSIS copy. Either way the MSI-installed app removes the NSIS side once it
//! runs: an NSIS install in its own directory loses just its registration,
//! uninstaller and the per-user shortcuts the MSI duplicates; one elsewhere
//! runs its own silent uninstaller.

#[cfg(any(windows, test))]
use std::path::{Path, PathBuf};

/// Runs the hand-over off the startup path; a no-op outside Windows.
pub fn spawn() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        if let Err(error) = windows::migrate() {
            eprintln!("NSIS install hand-over failed: {error}");
        }
    });
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// The MSI installed over the NSIS directory and owns its files now.
    Unregister { uninstaller: PathBuf },
    /// The NSIS install lives elsewhere and removes itself.
    Uninstall { uninstaller: PathBuf },
}

/// Decides what an NSIS registration calls for. Only the app an MSI installed
/// acts: a dev build, or an NSIS build of its own, never matches an MSI
/// install location. Callers canonicalize every path the same way first.
#[cfg(any(windows, test))]
fn plan(exe_dir: &Path, msi_locations: &[PathBuf], nsis_dir: Option<&Path>) -> Option<Plan> {
    if !msi_locations
        .iter()
        .any(|location| same_dir(location, exe_dir))
    {
        return None;
    }
    let nsis_dir = nsis_dir?;
    let uninstaller = nsis_dir.join("uninstall.exe");
    // Files an MSI owns, ours included, never meet the NSIS uninstaller.
    let owned_by_msi = msi_locations
        .iter()
        .any(|location| same_dir(location, nsis_dir));
    Some(if owned_by_msi {
        Plan::Unregister { uninstaller }
    } else {
        Plan::Uninstall { uninstaller }
    })
}

/// NSIS writes InstallLocation quoted.
#[cfg(any(windows, test))]
fn nsis_dir(location: &str) -> Option<PathBuf> {
    let dir = location.trim().trim_matches('"');
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

/// Windows directory identity: case-insensitive, either separator, no
/// trailing separator or verbatim prefix.
#[cfg(any(windows, test))]
fn same_dir(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        text.trim_end_matches('\\').to_string()
    };
    key(a) == key(b)
}

#[cfg(windows)]
mod windows {
    use super::{nsis_dir, plan, Plan};
    use std::path::{Path, PathBuf};
    use std::ptr::null_mut;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
        MsiEnumRelatedProductsW, MsiGetProductInfoW, INSTALLPROPERTY_INSTALLLOCATION,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::Registry::{
        RegDeleteTreeW, RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_PublicDesktop,
        SHGetKnownFolderPath,
    };

    /// Tauri's default `uuid5(DNS, "<productName>.exe.app.x64")`, unchanged
    /// since the first MSI.
    const UPGRADE_CODE: &str = "{A5829978-F7B9-5882-B5BB-B8521FCEBF5F}";
    const NSIS_UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Yuzora";
    const SHORTCUT: &str = "Yuzora.lnk";
    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;
    const ERROR_FILE_NOT_FOUND: u32 = 2;

    pub(super) fn migrate() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        let Some(exe_dir) = exe.parent() else {
            return Ok(());
        };
        let Some(nsis_dir) = registry_string(NSIS_UNINSTALL_KEY, "InstallLocation")
            .and_then(|value| nsis_dir(&value))
        else {
            return Ok(());
        };
        // Canonical like the exe and MSI paths, so a subst drive, junction or
        // short name still reads as the directory the MSI took over.
        let nsis_dir = std::fs::canonicalize(&nsis_dir).unwrap_or(nsis_dir);
        match plan(exe_dir, &msi_install_locations(), Some(&nsis_dir)) {
            None => Ok(()),
            Some(Plan::Unregister { uninstaller }) => {
                delete_nsis_registration()?;
                match std::fs::remove_file(&uninstaller) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        eprintln!("NSIS uninstaller kept: {error}");
                    }
                    _ => {}
                }
                remove_duplicate_shortcut(&FOLDERID_Programs, &FOLDERID_CommonPrograms, "Yuzora");
                remove_duplicate_shortcut(&FOLDERID_Desktop, &FOLDERID_PublicDesktop, "");
                Ok(())
            }
            // Its uninstaller removes only its own files, shortcuts and
            // registration; app data stays because silent mode never asks.
            Some(Plan::Uninstall { uninstaller }) if uninstaller.is_file() => {
                std::process::Command::new(&uninstaller)
                    .arg("/S")
                    .spawn()
                    .map(drop)
                    .map_err(|error| format!("{}: {error}", uninstaller.display()))
            }
            // Its files are already gone; only the registration is left.
            Some(Plan::Uninstall { .. }) => delete_nsis_registration(),
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn registry_string(key: &str, value: &str) -> Option<String> {
        let (key, value) = (wide(key), wide(value));
        let mut bytes = 0u32;
        let probe = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                &mut bytes,
            )
        };
        if probe != ERROR_SUCCESS || bytes == 0 {
            return None;
        }
        let mut buffer = vec![0u16; bytes as usize / 2];
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        (read == ERROR_SUCCESS).then(|| from_wide(&buffer))
    }

    fn delete_nsis_registration() -> Result<(), String> {
        let key = wide(NSIS_UNINSTALL_KEY);
        match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            code => Err(format!("delete NSIS registration: Win32 error {code}")),
        }
    }

    fn msi_install_locations() -> Vec<PathBuf> {
        let upgrade = wide(UPGRADE_CODE);
        let mut locations = Vec::new();
        for index in 0.. {
            let mut product = [0u16; 39];
            let found = unsafe {
                MsiEnumRelatedProductsW(upgrade.as_ptr(), 0, index, product.as_mut_ptr())
            };
            if found != ERROR_SUCCESS {
                break;
            }
            let mut chars = 0u32;
            let probe = unsafe {
                MsiGetProductInfoW(
                    product.as_ptr(),
                    INSTALLPROPERTY_INSTALLLOCATION,
                    null_mut(),
                    &mut chars,
                )
            };
            if (probe != ERROR_SUCCESS && probe != ERROR_MORE_DATA) || chars == 0 {
                continue;
            }
            chars += 1;
            let mut buffer = vec![0u16; chars as usize];
            let read = unsafe {
                MsiGetProductInfoW(
                    product.as_ptr(),
                    INSTALLPROPERTY_INSTALLLOCATION,
                    buffer.as_mut_ptr(),
                    &mut chars,
                )
            };
            if read == ERROR_SUCCESS {
                let location = PathBuf::from(from_wide(&buffer));
                locations.push(std::fs::canonicalize(&location).unwrap_or(location));
            }
        }
        locations
    }

    /// Drops NSIS's per-user shortcut where the MSI placed the same one for
    /// all users, so a working shortcut always remains.
    fn remove_duplicate_shortcut(user: &GUID, common: &GUID, common_subfolder: &str) {
        let (Some(user), Some(common)) = (known_folder(user), known_folder(common)) else {
            return;
        };
        if common.join(common_subfolder).join(SHORTCUT).is_file() {
            let _ = std::fs::remove_file(user.join(SHORTCUT));
        }
    }

    fn known_folder(id: &GUID) -> Option<PathBuf> {
        let mut path = null_mut();
        let result = unsafe { SHGetKnownFolderPath(id, 0, null_mut(), &mut path) };
        let folder = (result >= 0 && !path.is_null()).then(|| {
            let len = (0..).take_while(|&i| unsafe { *path.add(i) } != 0).count();
            PathBuf::from(String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(path, len)
            }))
        });
        unsafe { CoTaskMemFree(path.cast_const().cast()) };
        folder.filter(|folder| Path::new(folder).is_dir())
    }

    fn from_wide(buffer: &[u16]) -> String {
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSI: &str = r"C:\Users\u\AppData\Local\Yuzora";
    const NSIS: &str = "\"C:\\Users\\u\\AppData\\Local\\Yuzora\"";

    fn msi() -> Vec<PathBuf> {
        vec![PathBuf::from(MSI)]
    }

    fn uninstaller() -> PathBuf {
        PathBuf::from(MSI).join("uninstall.exe")
    }

    #[test]
    fn reads_the_quoted_nsis_install_location() {
        assert_eq!(nsis_dir(NSIS), Some(PathBuf::from(MSI)));
        assert_eq!(nsis_dir(" \"\" "), None);
        assert_eq!(nsis_dir(""), None);
    }

    #[test]
    fn unregisters_an_nsis_install_the_msi_took_over() {
        let exe_dir = Path::new(r"\\?\C:\users\u\appdata\local\yuzora\");
        assert_eq!(
            plan(exe_dir, &msi(), nsis_dir(NSIS).as_deref()),
            Some(Plan::Unregister {
                uninstaller: uninstaller()
            })
        );
    }

    #[test]
    fn uninstalls_an_nsis_install_left_beside_the_msi() {
        let program_files = r"C:\Program Files\Yuzora";
        assert_eq!(
            plan(
                Path::new(program_files),
                &[PathBuf::from(program_files)],
                nsis_dir(NSIS).as_deref()
            ),
            Some(Plan::Uninstall {
                uninstaller: uninstaller()
            })
        );
    }

    #[test]
    fn never_runs_the_nsis_uninstaller_over_msi_files() {
        // Another MSI product sits in the NSIS directory.
        let program_files = PathBuf::from(r"C:\Program Files\Yuzora");
        assert_eq!(
            plan(
                &program_files,
                &[program_files.clone(), PathBuf::from(MSI)],
                nsis_dir(NSIS).as_deref()
            ),
            Some(Plan::Unregister {
                uninstaller: uninstaller()
            })
        );
    }

    #[test]
    fn leaves_nsis_alone_unless_running_from_an_msi_install() {
        let nsis = nsis_dir(NSIS);
        // An NSIS build, or a dev build, with no MSI behind it.
        assert_eq!(plan(Path::new(MSI), &[], nsis.as_deref()), None);
        assert_eq!(
            plan(
                Path::new(r"D:\src\yuzora\target\debug"),
                &msi(),
                nsis.as_deref()
            ),
            None
        );
        // Nothing registered by NSIS.
        assert_eq!(plan(Path::new(MSI), &msi(), None), None);
    }

    #[test]
    fn directory_identity_ignores_case_separators_and_verbatim_prefix() {
        assert!(same_dir(Path::new(r"C:\A\b\"), Path::new("c:/a/B")));
        assert!(same_dir(Path::new(r"\\?\C:\A"), Path::new(r"C:\a")));
        assert!(!same_dir(Path::new(r"C:\A"), Path::new(r"C:\A\B")));
    }
}
