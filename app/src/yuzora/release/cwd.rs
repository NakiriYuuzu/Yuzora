//! Windows will not rename a folder while any program has it as its working
//! directory (os error 32), and an update renames the install folder
//! (`swap` in src/update.rs). The Start menu shortcut starts Yuzora in that
//! folder, so does the installer's "open Yuzora", and everything Yuzora starts
//! inherits it: herdr-core outlives the app and would hold the folder for good.
//! So the app leaves the folder before it starts anything.

use std::path::{Path, PathBuf};

/// Windows: moves this process's working directory to the home folder when it
/// is the folder this program is in (or inside it). Call it before anything
/// is started, and only for a bare launch: a command line may name files
/// relative to where it was typed.
pub fn leave_install_dir() {
    if !cfg!(windows) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(from) = leave(&exe, &crate::mods::home()) {
        tracing::info!(
            "working directory moved from {} to the home folder",
            from.display()
        );
    }
}

/// Returns the working directory it left.
fn leave(exe: &Path, home: &Path) -> Option<PathBuf> {
    // Both sides canonical: `current_dir()` says `C:\…` where a canonical exe path says `\\?\C:\…`.
    let exe = exe.canonicalize().ok()?;
    let install = exe.parent()?;
    let cwd = std::env::current_dir().ok()?.canonicalize().ok()?;
    if !cwd.starts_with(install) {
        return None;
    }
    match std::env::set_current_dir(home) {
        Ok(()) => Some(cwd),
        Err(e) => {
            tracing::warn!(
                "cannot move out of {} to {}: {e}",
                cwd.display(),
                home.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const CHILD: &str = "YUZORA_CWD_TEST";

    /// The other half of `run`: leaves the way `leave_install_dir` does, then says where it is.
    #[test]
    fn child() {
        let Ok(spec) = std::env::var(CHILD) else {
            return;
        };
        let (exe, home) = spec.split_once('\n').unwrap();
        let left = leave(Path::new(exe), Path::new(home));
        println!(
            "left={}",
            left.map_or("-".into(), |p| p.display().to_string())
        );
        println!("now={}", std::env::current_dir().unwrap().display());
    }

    /// `leave` in a process of its own, started in `cwd`: the working directory belongs to the process, and the other tests share this one.
    fn run(cwd: &Path, exe: &Path, home: &Path) -> (Option<PathBuf>, PathBuf) {
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "yuzora::release::cwd::tests::child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, format!("{}\n{}", exe.display(), home.display()))
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        // The harness prints "test … ... " before the child's first line.
        let field = |name: &str| {
            text.lines()
                .find_map(|l| l.split_once(name).map(|(_, v)| v))
                .unwrap_or_else(|| panic!("no {name} in {text}"))
                .to_owned()
        };
        let left = field("left=");
        (
            (left != "-").then(|| PathBuf::from(left)),
            PathBuf::from(field("now=")),
        )
    }

    struct Folders {
        base: PathBuf,
        install: PathBuf,
        exe: PathBuf,
        home: PathBuf,
    }

    impl Folders {
        fn new(name: &str) -> Folders {
            let base = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("yuzora-cwd-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let install = base.join("real/Yuzora");
            for dir in [
                install.join("sub"),
                base.join("real/Yuzora.new"),
                base.join("home"),
                base.join("elsewhere"),
            ] {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(install.join("yuzora.exe"), "x").unwrap();
            Folders {
                exe: install.join("yuzora.exe"),
                home: base.join("home"),
                install,
                base,
            }
        }
    }

    impl Drop for Folders {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn a_working_directory_in_the_install_folder_moves_to_home() {
        let f = Folders::new("inside");
        for cwd in [f.install.clone(), f.install.join("sub")] {
            let (left, now) = run(&cwd, &f.exe, &f.home);
            assert_eq!(left, Some(cwd.clone()));
            assert_eq!(now, f.home);
        }
    }

    #[test]
    fn a_folder_beside_the_install_folder_stays_put() {
        let f = Folders::new("beside");
        for cwd in [
            f.base.join("real/Yuzora.new"),
            f.base.join("elsewhere"),
            f.base.join("real"),
        ] {
            let (left, now) = run(&cwd, &f.exe, &f.home);
            assert_eq!((left, now), (None, cwd));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_program_named_through_a_link_is_still_found() {
        let f = Folders::new("link");
        let link = f.base.join("link");
        std::os::unix::fs::symlink(f.base.join("real"), &link).unwrap();
        let (left, now) = run(&f.install, &link.join("Yuzora/yuzora.exe"), &f.home);
        assert_eq!((left, now), (Some(f.install.clone()), f.home.clone()));
    }
}
