//! `~/.local/bin/yuzora` for an installed Yuzora.app (upstream's
//! src/core_bin/cli_link.rs does the same for Herdr.app and `herdr-app`).
//! Only a link that already points at a Yuzora.app is ever replaced: a file,
//! or a link to anything else, is the person's and stays.

use std::io;
use std::path::Path;

fn yuzora_bundle(exe: &Path) -> bool {
    exe.is_absolute() && exe.ends_with("Yuzora.app/Contents/MacOS/Yuzora")
}

/// Not run from the disk image, a translocated copy, or a build folder.
fn installed_bundle(exe: &Path) -> bool {
    yuzora_bundle(exe)
        && !exe.starts_with("/Volumes")
        && !exe.components().any(|c| c.as_os_str() == "AppTranslocation" || c.as_os_str() == "target")
}

pub(super) fn ensure(home: &Path, exe: &Path) -> io::Result<()> {
    if !installed_bundle(exe) {
        return Ok(());
    }
    let link = home.join(".local/bin/yuzora");
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(&link)?;
            if target == exe {
                return Ok(());
            }
            let resolved = if target.is_absolute() { target } else { link.parent().unwrap().join(target) };
            if !yuzora_bundle(&resolved) {
                return Ok(());
            }
            std::fs::remove_file(&link)?;
        }
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(link.parent().unwrap())?;
    std::os::unix::fs::symlink(exe, link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn makes_and_repairs_only_yuzoras_link() {
        let home = std::env::temp_dir().join(format!("yuzora-cli-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let exe = home.join("Applications/Yuzora.app/Contents/MacOS/Yuzora");
        let link = home.join(".local/bin/yuzora");
        ensure(&home, &exe).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        ensure(&home, &exe).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        assert!(!home.join(".local/bin/herdr-app").exists(), "never the official Herdr's CLI");

        // An old Yuzora.app (moved, the disk image, translocated) gets repointed.
        for old in [
            home.join("old/Yuzora.app/Contents/MacOS/Yuzora"),
            "../../Applications/Yuzora.app/Contents/MacOS/Yuzora".into(),
            "/Volumes/Yuzora/Yuzora.app/Contents/MacOS/Yuzora".into(),
            "/private/var/folders/AppTranslocation/x/d/Yuzora.app/Contents/MacOS/Yuzora".into(),
        ] {
            std::fs::remove_file(&link).unwrap();
            symlink(old, &link).unwrap();
            ensure(&home, &exe).unwrap();
            assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        }

        // Anything else stays: another program's link, a file of the person's.
        std::fs::remove_file(&link).unwrap();
        symlink("/usr/local/bin/something-else", &link).unwrap();
        ensure(&home, &exe).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("/usr/local/bin/something-else"));
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "personal yuzora script").unwrap();
        ensure(&home, &exe).unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "personal yuzora script");

        // Runs from the image, a translocated copy or cargo's target/ make no link.
        std::fs::remove_file(&link).unwrap();
        for running in ["/Volumes/Yuzora/Yuzora.app/Contents/MacOS/Yuzora", "/private/var/folders/AppTranslocation/x/d/Yuzora.app/Contents/MacOS/Yuzora", "/src/app/target/Yuzora.app/Contents/MacOS/Yuzora", "/Applications/Herdr.app/Contents/MacOS/Herdr"] {
            ensure(&home, Path::new(running)).unwrap();
            assert!(std::fs::symlink_metadata(&link).is_err(), "{running}");
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
