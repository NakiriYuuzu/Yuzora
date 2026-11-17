//! Where a link to a mod or layout points: a GitHub repository, maybe a
//! branch or commit, maybe a folder in it. Four forms are read:
//!
//! - `https://github.com/<owner>/<repo>`
//! - `https://github.com/<owner>/<repo>/tree/<ref>/<path>`
//! - `<owner>/<repo>`
//! - `herdr.dev/mods/<owner>/<slug>` and `herdr.dev/layouts/<owner>/<slug>`:
//!   the gallery names a repo's folder as `<repo>--<folder>--<folder>`.
//!
//! herdr.dev is only read here, as text: the files always come from GitHub.

/// Where GitHub serves a repository's files as one archive.
pub const CODELOAD: &str = "https://codeload.github.com";

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub owner: String,
    pub repo: String,
    /// A branch, tag or commit; none is the default branch (`HEAD`).
    pub reference: Option<String>,
    /// The folder in the repository, one name per level; empty is its root.
    pub path: Vec<String>,
}

impl Source {
    /// The archive of this source's commit: GitHub's legacy archive, whose one
    /// folder is `<owner>-<repo>-<commit>`.
    pub fn archive_url(&self, base: &str) -> String {
        format!("{base}/{}/{}/legacy.tar.gz/{}", self.owner, self.repo, self.reference.as_deref().unwrap_or("HEAD"))
    }

    /// The address a person opens to read it.
    pub fn github_url(&self) -> String {
        let repo = format!("https://github.com/{}/{}", self.owner, self.repo);
        if self.reference.is_none() && self.path.is_empty() {
            return repo;
        }
        let tree = format!("{repo}/tree/{}", self.reference.as_deref().unwrap_or("HEAD"));
        if self.path.is_empty() { tree } else { format!("{tree}/{}", self.path.join("/")) }
    }

    /// What the folder it names is called: the last folder, else the repo.
    pub fn name(&self) -> &str {
        self.path.last().unwrap_or(&self.repo)
    }
}

/// The archives' address: `YUZORA_INSTALL_URL` (`var`) in a debug build, for
/// tests against a server of their own; GitHub otherwise.
pub fn base(var: Option<String>, debug: bool) -> String {
    match var.filter(|v| debug && !v.trim().is_empty()) {
        Some(v) => v.trim().trim_end_matches('/').to_owned(),
        None => CODELOAD.to_owned(),
    }
}

/// `None`: not one of the four forms, or a name GitHub wouldn't have.
pub fn parse(input: &str) -> Option<Source> {
    let text = input.trim().split(['?', '#']).next()?;
    let lower = text.to_ascii_lowercase();
    let scheme = ["https://", "http://"].iter().find(|s| lower.starts_with(*s)).map_or(0, |s| s.len());
    let host = |name: &str| ["", "www."].iter().map(|w| format!("{w}{name}/")).find(|h| lower[scheme..].starts_with(h.as_str())).map(|h| scheme + h.len());
    let after = |at: usize| -> Option<Vec<&str>> {
        let parts: Vec<&str> = text[at..].strip_suffix('/').unwrap_or(&text[at..]).split('/').collect();
        parts.iter().all(|p| !p.is_empty()).then_some(parts)
    };
    let source = if let Some(at) = host("github.com") {
        match after(at)?.as_slice() {
            [owner, repo] => repo_source(owner, repo, None, &[])?,
            [owner, repo, "tree", reference, path @ ..] => repo_source(owner, repo, Some(reference), path)?,
            _ => return None,
        }
    } else if let Some(at) = host("herdr.dev") {
        match after(at)?.as_slice() {
            ["mods" | "layouts", owner, slug] => {
                let mut parts = slug.split("--");
                let repo = parts.next()?;
                let path: Vec<&str> = parts.collect();
                repo_source(owner, repo, None, &path)?
            }
            _ => return None,
        }
    } else if scheme == 0 {
        match after(0)?.as_slice() {
            [owner, repo] => repo_source(owner, repo, None, &[])?,
            _ => return None,
        }
    } else {
        return None;
    };
    Some(source)
}

fn repo_source(owner: &str, repo: &str, reference: Option<&str>, path: &[&str]) -> Option<Source> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let name = |s: &str, extra: &[char]| s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(&c));
    let owner_ok = (1..=39).contains(&owner.len()) && name(owner, &['-']) && !owner.starts_with('-') && !owner.ends_with('-');
    let repo_ok = (1..=100).contains(&repo.len()) && name(repo, &['-', '_', '.']) && !repo.starts_with('-') && repo != "." && repo != "..";
    let reference_ok = reference.is_none_or(|r| {
        (1..=255).contains(&r.len())
            && name(r, &['-', '_', '.', '/'])
            && !r.starts_with(['-', '/'])
            && !r.ends_with(['/', '.'])
            && !r.contains("..")
            && !r.contains("//")
    });
    let path_ok = path.iter().all(|p| !p.is_empty() && name(p, &['-', '_', '.']) && !p.starts_with('-') && *p != "." && *p != "..");
    (owner_ok && repo_ok && reference_ok && path_ok).then(|| Source {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference: reference.map(str::to_owned),
        path: path.iter().map(|p| (*p).to_owned()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(owner: &str, repo: &str, reference: Option<&str>, path: &[&str]) -> Option<Source> {
        Some(Source { owner: owner.into(), repo: repo.into(), reference: reference.map(Into::into), path: path.iter().map(|p| (*p).into()).collect() })
    }

    #[test]
    fn reads_the_four_forms() {
        assert_eq!(parse("https://github.com/acme/demo"), src("acme", "demo", None, &[]));
        assert_eq!(parse("https://github.com/acme/kit/tree/main/release/blackx"), src("acme", "kit", Some("main"), &["release", "blackx"]));
        assert_eq!(parse("https://github.com/acme/kit/tree/v1.2"), src("acme", "kit", Some("v1.2"), &[]));
        assert_eq!(parse("acme/demo"), src("acme", "demo", None, &[]));
        assert_eq!(parse("herdr.dev/mods/acme/kit--release--blackx"), src("acme", "kit", None, &["release", "blackx"]));
        assert_eq!(parse("https://herdr.dev/layouts/acme/deck"), src("acme", "deck", None, &[]));
    }

    #[test]
    fn hosts_ignore_case_and_names_keep_theirs() {
        assert_eq!(parse("HTTPS://GitHub.com/Acme/Demo"), src("Acme", "Demo", None, &[]));
        assert_eq!(parse("http://www.github.com/acme/demo.git"), src("acme", "demo", None, &[]));
        assert_eq!(parse("HERDR.dev/mods/Acme/Kit--Blackx"), src("Acme", "Kit", None, &["Blackx"]));
    }

    #[test]
    fn a_trailing_slash_a_query_or_spaces_change_nothing() {
        let demo = src("acme", "demo", None, &[]);
        for input in ["https://github.com/acme/demo/", "  acme/demo \n", "https://github.com/acme/demo?tab=readme-ov-file#install", "herdr.dev/layouts/acme/demo/"] {
            assert_eq!(parse(input), demo, "{input:?}");
        }
    }

    #[test]
    fn refuses_what_is_not_a_repo() {
        for input in [
            "",
            "acme",
            "acme/demo/extra",
            "https://gitlab.com/acme/demo",
            "https://github.com.evil.example/acme/demo",
            "https://evil.example/github.com/acme/demo",
            "https://herdr.dev/acme/demo",
            "https://herdr.dev/mods/acme",
            "https://github.com/acme/demo/blob/main/README.md",
            "https://github.com/acme//demo",
            "https://github.com/acme/kit/tree/main/../../etc",
            "https://github.com/acme/kit/tree/../x",
            "herdr.dev/mods/acme/kit--..",
            "herdr.dev/mods/acme/kit----x",
            "herdr.dev/mods/acme/kit--",
            "-oevil/demo",
            "acme/-demo",
            "acme-/demo",
            "acme/..",
            "acme/.",
            "acme/de mo",
            "acme/$(touch x)",
            "acme/demo;rm",
            "https://github.com/acme/kit/tree/-x",
            "https://github.com/acme/kit/tree/main/-x",
            "https://github.com/acme/kit/tree/a%20b",
            "https://github.com/acme/{a,b}",
            "file:///etc/passwd",
        ] {
            assert_eq!(parse(input), None, "{input:?}");
        }
    }

    #[test]
    fn asks_github_and_shows_the_github_address() {
        let kit = parse("herdr.dev/mods/acme/kit--release--blackx").unwrap();
        assert_eq!(kit.archive_url(CODELOAD), "https://codeload.github.com/acme/kit/legacy.tar.gz/HEAD");
        assert_eq!(kit.github_url(), "https://github.com/acme/kit/tree/HEAD/release/blackx");
        assert_eq!(kit.name(), "blackx");
        let demo = parse("acme/demo").unwrap();
        assert_eq!(demo.archive_url(CODELOAD), "https://codeload.github.com/acme/demo/legacy.tar.gz/HEAD");
        assert_eq!(demo.github_url(), "https://github.com/acme/demo");
        assert_eq!(demo.name(), "demo");
        let branch = parse("https://github.com/acme/kit/tree/dev/mods").unwrap();
        assert_eq!(branch.archive_url(CODELOAD), "https://codeload.github.com/acme/kit/legacy.tar.gz/dev");
        assert_eq!(branch.github_url(), "https://github.com/acme/kit/tree/dev/mods");
    }

    #[test]
    fn only_a_debug_build_reads_the_override() {
        let local = Some("http://127.0.0.1:9/".to_owned());
        assert_eq!(base(local.clone(), true), "http://127.0.0.1:9");
        assert_eq!(base(local, false), CODELOAD, "a release build always asks GitHub");
        assert_eq!(base(None, true), CODELOAD);
        assert_eq!(base(Some(" ".into()), true), CODELOAD);
    }
}
