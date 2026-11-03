//! Lane L2c: trusted folders for the git the app runs on its own outside
//! `yuzora:git`. A repository's own config can name programs git starts
//! while it reads the index or the work tree (a clean filter on a touched
//! file, an fsmonitor hook). Until the person trusts the folder (the Git
//! sidebar asks; `yuzora_core::git_trust` keeps the list), those reads
//! don't run here. Reads of refs only (`rev-parse --show-toplevel`,
//! `symbolic-ref`, …) run anyway: docs/beta2/git.md lists every place,
//! under "`yuzora:git` 以外的入口".
//!
//! Only the Linux folder watch reads a work tree today (`files.rs`, its
//! ignored folders).
#![cfg(any(target_os = "linux", test))]

use std::io;
use std::path::Path;
use std::process::{Command, Output};
use yuzora_core::git_trust;

/// `git -C <folder> <args>` for a command that reads the index or the work
/// tree (`ls-files --others`, `status`, `diff`), once nothing in the
/// folder's repository waits for trust. Else an error, and only
/// `rev-parse --show-toplevel` ran (it reads neither).
#[cfg(target_os = "linux")]
pub fn in_work_tree(folder: &Path, args: &[&str]) -> io::Result<Output> {
    in_work_tree_at(&git_trust::trusted_file(), folder, args)
}

/// [`in_work_tree`] with the trusted folders in `file`.
fn in_work_tree_at(file: &Path, folder: &Path, args: &[&str]) -> io::Result<Output> {
    let top = Command::new("git").arg("-C").arg(folder).args(git_trust::READ_FLAGS).args(["rev-parse", "--show-toplevel"]).output()?;
    if !top.status.success() {
        return Err(io::Error::other(format!("{}: not in a work tree", folder.display())));
    }
    // The folder id is the top as git prints it; a name git can't print as text is never trusted.
    let top = String::from_utf8(top.stdout).map_err(io::Error::other)?;
    let top = top.strip_suffix('\n').unwrap_or(&top);
    match git_trust::pending_from(file, None, top) {
        Ok(waits) if waits.is_empty() => Command::new("git").arg("-C").arg(folder).args(args).output(),
        Ok(waits) => Err(io::Error::other(format!("untrusted folder {top} ({})", waits.join(", ")))),
        Err(e) => Err(io::Error::other(e)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// What `files.rs` asks for a folder watch on Linux.
    const IGNORED: [&str; 6] = ["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"];

    /// A repo whose own config names a clean and a smudge filter for `*.txt`
    /// and an fsmonitor hook, each leaving `ran-<name>` behind; `a.txt` is
    /// committed and touched since, so a status hashes it again. Its git
    /// setup sees a home of its own, none of this computer's config.
    struct Fixture {
        dir: PathBuf,
        repo: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("yuzora-l2c-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (home, repo) = (dir.join("home"), dir.join("repo"));
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(repo.join("ignored")).unwrap();
            std::fs::write(home.join(".gitconfig"), "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n").unwrap();
            let repo = repo.canonicalize().unwrap();
            let f = Self { dir: dir.canonicalize().unwrap(), repo };
            f.git(&["init", "-q"]);
            std::fs::write(f.repo.join("a.txt"), "1\n").unwrap();
            std::fs::write(f.repo.join(".gitignore"), "ignored/\n").unwrap();
            f.git(&["add", "a.txt", ".gitignore"]);
            f.git(&["commit", "-qm", "one"]);
            for (key, name, then) in [("filter.evil.clean", "clean", "cat"), ("filter.evil.smudge", "smudge", "cat"), ("core.fsmonitor", "fsmonitor", "exit 1")] {
                let program = f.dir.join(format!("{name}.sh"));
                std::fs::write(&program, format!("#!/bin/sh\ntouch '{}'\n{then}\n", f.dir.join(format!("ran-{name}")).display())).unwrap();
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
                f.git(&["config", key, &program.to_string_lossy()]);
            }
            std::fs::write(f.repo.join(".git/info/attributes"), "*.txt filter=evil\n").unwrap();
            f.touch();
            f
        }

        fn git(&self, args: &[&str]) {
            let ok = Command::new("git").args(args).current_dir(&self.repo).env("HOME", self.dir.join("home")).env("GIT_CONFIG_NOSYSTEM", "1").status().unwrap().success();
            assert!(ok, "git {args:?}");
        }

        /// The same content with a new mtime: status hashes it again.
        fn touch(&self) {
            std::fs::write(self.repo.join("a.txt"), "1\n").unwrap();
            let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
            std::fs::File::options().write(true).open(self.repo.join("a.txt")).unwrap().set_modified(past).unwrap();
        }

        /// The programs that ran since the last look.
        fn ran(&self) -> Vec<&'static str> {
            let ran: Vec<&str> = ["clean", "smudge", "fsmonitor"].into_iter().filter(|n| self.dir.join(format!("ran-{n}")).exists()).collect();
            for n in &ran {
                let _ = std::fs::remove_file(self.dir.join(format!("ran-{n}")));
            }
            ran
        }

        fn trusted(&self) -> PathBuf {
            self.dir.join("trusted.json")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn git_here() -> bool {
        Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    #[test]
    fn an_untrusted_folder_runs_none_of_its_repos_programs_then_trusted_runs_as_before() {
        if !git_here() {
            return;
        }
        let f = Fixture::new("gate");
        assert_eq!(f.ran(), Vec::<&str>::new());
        let status = ["status", "--porcelain"];
        for args in [&IGNORED[..], &status[..]] {
            let e = in_work_tree_at(&f.trusted(), &f.repo, args).unwrap_err();
            assert!(e.to_string().contains("untrusted folder") && e.to_string().contains("filter.evil.clean"), "{e}");
            assert_eq!(f.ran(), Vec::<&str>::new(), "{args:?} ran a program before the folder was trusted");
        }
        // A folder below the top counts as the repo's.
        assert!(in_work_tree_at(&f.trusted(), &f.repo.join("ignored"), &IGNORED).is_err());
        assert_eq!(f.ran(), Vec::<&str>::new());

        let top = f.repo.to_string_lossy().into_owned();
        git_trust::trust_at(&f.trusted(), &git_trust::folder_id(None, &top)).unwrap();
        let out = in_work_tree_at(&f.trusted(), &f.repo, &IGNORED).unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ignored/\0");
        // As before L2c: git runs the hook its config names, so the fixture is a real one.
        assert_eq!(f.ran(), vec!["fsmonitor"]);
        f.touch();
        assert!(in_work_tree_at(&f.trusted(), &f.repo, &status).unwrap().status.success());
        assert!(f.ran().contains(&"clean"), "git status didn't run the clean filter, so this test proves nothing");
    }

    /// The Linux folder watch a mod starts (`watchFolder`) on an untrusted
    /// repo: its ignored folders are left unknown rather than read.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_watch_on_an_untrusted_repo_runs_none_of_its_programs() {
        use std::future::Future as _;
        if !git_here() {
            return;
        }
        let f = Fixture::new("watch");
        let mut wait = Box::pin(crate::files::next_change(f.repo.to_string_lossy().into_owned()));
        assert!(wait.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())).is_pending());
        assert_eq!(f.ran(), Vec::<&str>::new(), "the folder watch ran a program of an untrusted repo");
    }

    #[test]
    fn the_folder_watch_reads_ignored_folders_through_the_gate() {
        assert!(include_str!("../files.rs").contains(r#"crate::yuzora::trust::in_work_tree(root, &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"])"#));
    }

    #[test]
    fn outside_a_repo_nothing_runs() {
        if !git_here() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("yuzora-l2c-plain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = in_work_tree_at(&dir.join("trusted.json"), &dir, &IGNORED).unwrap_err();
        assert!(e.to_string().contains("not in a work tree"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The git the app and its mods run in any folder without asking: refs
    /// only. Each runs none of the fixture's programs, untrusted.
    #[test]
    fn the_refs_only_reads_run_nothing_in_an_untrusted_folder() {
        if !git_here() {
            return;
        }
        let f = Fixture::new("refs");
        // yuzora-shell's gitStatus (the spaces mod, the status bar) has its own test.
        let reads: [&[&str]; 5] = [
            // window.rs folder_info (a tab's folder and branch).
            &["rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"],
            // files.rs, the watch's git folders.
            &["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"],
            // The changes, files, menu, spaces and worktree mods' repo and checkout.
            &["rev-parse", "--show-toplevel"],
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            // defaults/AGENTS.md's example for a person's own mod.
            &["branch", "--show-current"],
        ];
        for args in reads {
            assert!(Command::new("git").args(args).current_dir(&f.repo).output().unwrap().status.success(), "{args:?}");
            assert_eq!(f.ran(), Vec::<&str>::new(), "{args:?} ran a program in an untrusted folder");
        }
        // The fixture is a real one: a status would have run them.
        Command::new("git").args(["status", "--porcelain"]).current_dir(&f.repo).output().unwrap();
        assert_eq!(f.ran(), vec!["clean", "fsmonitor"]);
        // What the app runs stays these (an upstream sync could change them).
        assert!(include_str!("../window.rs").contains(r#"["-C", cwd, "rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"]"#));
        assert!(include_str!("../files.rs").contains(r#".args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"])"#));
    }
}
