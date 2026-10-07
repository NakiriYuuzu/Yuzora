use super::{FileNameKind, FileNameMatch, FileNameSearchResult, MAX_ENTRIES, MAX_FILES};
use crate::path_capability::{NodeKind, PinnedDir, SafeLeafName};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// 24 retained DFS handles + the handed-off root and transient listing/ignore/
// open_subdir handles stay below 32 search-owned handles, leaving headroom even
// on hosts with a 64-fd soft limit. Never reopen ancestors by pathname.
const MAX_OPEN_FRAMES: usize = 24;

/// The display path is captured with the capability; it is never used for I/O.
pub(crate) struct PinnedSearchRoot {
    pub(crate) canonical: PathBuf,
    pub(crate) dir: PinnedDir,
}

// Like FilesList, a renamed root remains the original pinned directory, not the
// replacement at its display path. Successful scans can therefore still be complete.
pub(crate) fn run_pinned_file_name_search(
    root: &PinnedSearchRoot,
    query: &str,
    generation: u64,
    gen_source: &AtomicU64,
    time_budget: Duration,
) -> Result<FileNameSearchResult, String> {
    search(
        root,
        query,
        generation,
        gen_source,
        time_budget,
        &mut |_| {},
    )
}

struct Budget<'a> {
    started: Instant,
    time: Duration,
    remaining: usize,
    generation: u64,
    source: &'a AtomicU64,
}

impl Budget<'_> {
    fn current(&self) -> Result<(), String> {
        if self.source.load(Ordering::Relaxed) != self.generation {
            Err("file-name-search-cancelled".into())
        } else {
            Ok(())
        }
    }

    fn check(&self) -> Result<(), String> {
        self.current()?;
        if self.started.elapsed() >= self.time {
            Err("file-name-search-budget".into())
        } else {
            Ok(())
        }
    }

    fn entries(&mut self, dir: &PinnedDir) -> Result<Vec<(String, NodeKind)>, String> {
        self.check()?;
        let entries = dir.list_entries_until(self.remaining, || self.check().is_err())?;
        self.remaining -= entries.len();
        self.check()?;
        Ok(entries)
    }
}

struct Frame {
    dir: PinnedDir,
    relative: PathBuf,
    entries: std::vec::IntoIter<(String, NodeKind)>,
    gitignore: Gitignore,
    ignore: Gitignore,
    exclude: Gitignore,
}

impl Frame {
    fn new(
        dir: PinnedDir,
        relative: PathBuf,
        canonical: &Path,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        let entries = budget.entries(&dir)?;
        let path = canonical.join(&relative);
        let gitignore = read_ignore(&dir, ".gitignore", &path, budget)?;
        let ignore = read_ignore(&dir, ".ignore", &path, budget)?;
        let exclude = git_exclude(&dir, &path, budget)?;
        Ok(Self {
            dir,
            relative,
            entries: entries.into_iter(),
            gitignore,
            ignore,
            exclude,
        })
    }
}

fn read_ignore(
    dir: &PinnedDir,
    name: &str,
    base: &Path,
    budget: &Budget<'_>,
) -> Result<Gitignore, String> {
    let mut builder = GitignoreBuilder::new(base);
    budget.check()?;
    if let Some(opened) = dir.open_file_optional(&SafeLeafName::parse(name)?)? {
        let limit = crate::protocol::MAX_FILE_BYTES;
        if opened.len > limit {
            return Err("ignore-file-too-large".into());
        }
        let mut reader = BufReader::new(opened.file.take(limit + 1));
        let mut line = String::new();
        let mut bytes = 0;
        loop {
            budget.check()?;
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            bytes += read as u64;
            if bytes > limit {
                return Err("ignore-file-too-large".into());
            }
            let line = line.trim_end_matches(['\r', '\n']);
            // Match GitignoreBuilder::add's first-line UTF-8 BOM handling.
            let line = if bytes == read as u64 {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            builder
                .add_line(Some(base.join(name)), line)
                .map_err(|error| error.to_string())?;
        }
    }
    builder.build().map_err(|error| error.to_string())
}

fn git_exclude(
    dir: &PinnedDir,
    canonical: &Path,
    budget: &mut Budget<'_>,
) -> Result<Gitignore, String> {
    // Linked-worktree .git files and symlinks cannot grant access outside the capability.
    budget.check()?;
    if let Some(git) = dir.open_subdir_optional(&SafeLeafName::parse(".git")?)? {
        budget.check()?;
        if let Some(info) = git.open_subdir_optional(&SafeLeafName::parse("info")?)? {
            return read_ignore(&info, "exclude", canonical, budget);
        }
    }
    GitignoreBuilder::new(canonical)
        .build()
        .map_err(|error| error.to_string())
}

fn ignored(frames: &[Frame], path: &Path, directory: bool) -> bool {
    // ignore's precedence is file type first, then deepest directory; a parent
    // .ignore can override even a child's .gitignore (including whitelists).
    // As with the local walker's require_git(false), Git rules inherit even
    // across nested repository boundaries.
    for matcher in frames
        .iter()
        .rev()
        .map(|frame| &frame.ignore)
        .chain(frames.iter().rev().map(|frame| &frame.gitignore))
        .chain(frames.iter().rev().map(|frame| &frame.exclude))
    {
        let matched = matcher.matched(path, directory);
        if !matched.is_none() {
            return matched.is_ignore();
        }
    }
    false
}

fn search(
    root: &PinnedSearchRoot,
    query: &str,
    generation: u64,
    source: &AtomicU64,
    time_budget: Duration,
    before_child_open: &mut impl FnMut(&Path),
) -> Result<FileNameSearchResult, String> {
    let mut budget = Budget {
        started: Instant::now(),
        time: time_budget,
        remaining: MAX_ENTRIES,
        generation,
        source,
    };
    budget.current()?;
    let needle = query.trim().replace('\\', "/").to_lowercase();
    let mut result = FileNameSearchResult::default();
    if needle.is_empty() {
        return Ok(result);
    }
    let traversal = (|| -> Result<(), String> {
        let frame = Frame::new(
            root.dir.open_subdir("")?,
            PathBuf::new(),
            &root.canonical,
            &mut budget,
        )?;
        let mut frames = vec![frame];
        while !frames.is_empty() {
            budget.check()?;
            let frame = frames.last_mut().unwrap();
            let Some((name, kind)) = frame.entries.next() else {
                frames.pop();
                continue;
            };
            if name == ".git" || !matches!(kind, NodeKind::Directory | NodeKind::File) {
                continue;
            }
            let relative = frame.relative.join(&name);
            let path = root.canonical.join(&relative);
            if ignored(&frames, &path, kind == NodeKind::Directory) {
                continue;
            }
            if kind == NodeKind::Directory {
                if frames.len() >= MAX_OPEN_FRAMES {
                    result.incomplete = true;
                    continue;
                }
                before_child_open(&relative);
                let child = frames.last().unwrap().dir.open_subdir(&name)?;
                frames.push(Frame::new(child, relative, &root.canonical, &mut budget)?);
            } else {
                let normalized = relative
                    .components()
                    .map(|part| part.as_os_str().to_str())
                    .collect::<Option<Vec<_>>>();
                if !normalized.is_some_and(|parts| parts.join("/").to_lowercase().contains(&needle))
                {
                    continue;
                }
                let Some(path) = path.to_str() else { continue };
                result.files.push(FileNameMatch {
                    name,
                    path: path.to_owned(),
                    is_dir: false,
                    kind: FileNameKind::File,
                });
                if result.files.len() == MAX_FILES {
                    result.incomplete = true;
                    break;
                }
            }
        }
        Ok(())
    })();
    if traversal.is_err() {
        // A raced entry, unreadable ignore file or exhausted budget is not a complete scan.
        result.incomplete = true;
    }
    budget.current()?;
    result.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

#[cfg(test)]
mod tests;
