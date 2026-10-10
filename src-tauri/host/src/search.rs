use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const FILE_CAP: u32 = 200;
const TOTAL_MATCH_CAP: u32 = 2000;
const PER_FILE_MATCH_CAP: usize = 500;
const PREVIEW_LEN: usize = 200;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub line: u32,
    pub col: u32,
    pub preview: String,
    /// Matches inside `preview` as UTF-16 `[start, end)` offsets, found with the
    /// search's own matcher so case folding (Σ / ς, s / ſ) agrees. Older helpers
    /// omit it and the UI falls back to its own lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges: Option<Vec<[u32; 2]>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum SearchEvent {
    Match {
        path: String,
        matches: Vec<SearchMatch>,
    },
    Done {
        truncated: bool,
        file_count: u32,
    },
}

fn make_preview(line: &str) -> String {
    let trimmed = line.trim();
    trimmed.chars().take(PREVIEW_LEN).collect()
}

fn preview_ranges(matcher: &RegexMatcher, preview: &str) -> std::io::Result<Vec<[u32; 2]>> {
    let utf16 = |end: usize| {
        preview
            .get(..end)
            .map(|head| head.encode_utf16().count() as u32)
    };
    let mut ranges = Vec::new();
    matcher
        .find_iter(preview.as_bytes(), |found| {
            if let (false, Some(start), Some(end)) =
                (found.is_empty(), utf16(found.start()), utf16(found.end()))
            {
                ranges.push([start, end]);
            }
            true
        })
        .map_err(std::io::Error::other)?;
    Ok(ranges)
}

/// Collects matches for a single file. Stops the search and discards nothing
/// extra when binary data is detected — returning `false` from `binary_data`
/// makes the searcher quit before matching the truncated binary line. `budget`
/// is the per-file collection ceiling (the smaller of the per-file cap and the
/// remaining global match budget); enforcing it inside `matched` keeps a single
/// pathological file from buffering tens of thousands of matches before emit.
struct MatchCollector<'a> {
    matcher: &'a RegexMatcher,
    matches: Vec<SearchMatch>,
    budget: usize,
}

impl Sink for MatchCollector<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(mat.bytes());
        // Use the same Unicode case-folding rules as the searcher. Lowercasing
        // and searching again can discard valid matches (e.g. Σ / ς or s / ſ).
        if let Some(found) = self
            .matcher
            .find(mat.bytes())
            .map_err(std::io::Error::other)?
        {
            // Preserve the zero-based Unicode scalar column in the original
            // decoded line, even if lowercasing would expand a preceding char.
            let col = String::from_utf8_lossy(&mat.bytes()[..found.start()])
                .chars()
                .count() as u32;
            let preview = make_preview(&line);
            let ranges = preview_ranges(self.matcher, &preview)?;
            self.matches.push(SearchMatch {
                line: mat.line_number().unwrap_or(0) as u32,
                col,
                preview,
                ranges: Some(ranges),
            });
            if self.matches.len() >= self.budget {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn binary_data(
        &mut self,
        _searcher: &Searcher,
        _binary_byte_offset: u64,
    ) -> Result<bool, Self::Error> {
        Ok(false)
    }
}

pub fn run_search(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    generation: u64,
    gen_source: &AtomicU64,
    emit: &mut dyn FnMut(SearchEvent),
) {
    if query.is_empty() {
        emit(SearchEvent::Done {
            truncated: false,
            file_count: 0,
        });
        return;
    }

    let matcher = match RegexMatcherBuilder::new()
        .fixed_strings(true)
        .case_insensitive(!case_sensitive)
        .build(query)
    {
        Ok(m) => m,
        Err(_) => {
            emit(SearchEvent::Done {
                truncated: false,
                file_count: 0,
            });
            return;
        }
    };

    let mut file_count: u32 = 0;
    let mut total_matches: u32 = 0;
    let mut scanned = 0_u64;
    let mut incomplete = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    #[cfg(unix)]
    let pinned = match crate::path_capability::PinnedDir::open_dir(root) {
        Ok(pinned) => pinned,
        Err(_) => {
            emit(SearchEvent::Done {
                truncated: true,
                file_count: 0,
            });
            return;
        }
    };

    let mut searcher = None;
    for entry in WalkBuilder::new(root).require_git(false).build() {
        if gen_source.load(Ordering::Relaxed) != generation {
            return;
        }
        if Instant::now() >= deadline || scanned >= 512 * 1024 * 1024 {
            incomplete = true;
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        #[cfg(unix)]
        let file = entry
            .path()
            .strip_prefix(root)
            .ok()
            .and_then(|path| path.to_str())
            .and_then(|path| crate::path_capability::SafeRelativePath::parse(path).ok())
            .and_then(|path| pinned.open_file(&path).ok())
            .map(|file| file.file);
        #[cfg(not(unix))]
        let file = std::fs::File::open(entry.path()).ok();
        let Some(file) = file else {
            incomplete = true;
            continue;
        };
        let size = file
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(u64::MAX);
        if size > crate::protocol::MAX_FILE_BYTES {
            incomplete = true;
            continue;
        }
        scanned += size;
        let searcher = searcher.get_or_insert_with(|| {
            SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .build()
        });
        // Cap this file at the per-file ceiling but never past the remaining
        // global budget, so the total never overshoots TOTAL_MATCH_CAP.
        let budget = ((TOTAL_MATCH_CAP - total_matches) as usize).min(PER_FILE_MATCH_CAP);
        let mut collector = MatchCollector {
            matcher: &matcher,
            matches: Vec::new(),
            budget,
        };
        let _ = searcher.search_reader(
            &matcher,
            file.take(crate::protocol::MAX_FILE_BYTES),
            &mut collector,
        );
        if gen_source.load(Ordering::Relaxed) != generation {
            return;
        }

        if collector.matches.is_empty() {
            continue;
        }

        total_matches += collector.matches.len() as u32;
        emit(SearchEvent::Match {
            path: entry.path().to_string_lossy().into_owned(),
            matches: collector.matches,
        });
        file_count += 1;

        if total_matches >= TOTAL_MATCH_CAP || file_count >= FILE_CAP {
            emit(SearchEvent::Done {
                truncated: true,
                file_count,
            });
            return;
        }
    }

    emit(SearchEvent::Done {
        truncated: incomplete,
        file_count,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    fn collect(root: &std::path::Path, q: &str, cs: bool) -> Vec<SearchEvent> {
        let gen = AtomicU64::new(1);
        let mut out = Vec::new();
        run_search(root, q, cs, 1, &gen, &mut |e| out.push(e));
        out
    }

    #[test]
    fn finds_matches_with_line_and_col() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello\nworld hello\n").unwrap();
        let events = collect(tmp.path(), "hello", true);
        let SearchEvent::Match { path, matches } = &events[0] else {
            panic!()
        };
        assert!(path.ends_with("a.txt"));
        assert_eq!((matches[0].line, matches[0].col), (1, 0));
        assert_eq!(matches[1].line, 2);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: false,
                file_count: 1
            })
        ));
    }

    #[test]
    fn unicode_case_insensitive_matches_are_not_discarded() {
        for (line, query) in [("ς", "Σ"), ("ſ", "s"), ("s", "ſ")] {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(tmp.path().join("a.txt"), format!("{line}\n")).unwrap();
            let events = collect(tmp.path(), query, false);
            assert!(
                matches!(&events[0], SearchEvent::Match { matches, .. } if matches.len() == 1 && matches[0].col == 0),
                "missing Unicode match: {line:?} / {query:?}: {events:?}"
            );
            assert_eq!(collect(tmp.path(), query, true).len(), 1);
        }
    }

    #[test]
    fn case_insensitive_by_flag() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "Hello\n").unwrap();
        assert_eq!(collect(tmp.path(), "hello", true).len(), 1); // 只有 Done
        assert_eq!(collect(tmp.path(), "hello", false).len(), 2); // Match + Done
    }

    #[test]
    fn respects_gitignore_and_skips_binary() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(tmp.path().join("ignored.txt"), "needle\n").unwrap();
        std::fs::write(tmp.path().join("bin.dat"), b"needle\x00\x01").unwrap();
        std::fs::write(tmp.path().join("ok.txt"), "needle\n").unwrap();
        let events = collect(tmp.path(), "needle", true);
        let matched: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                SearchEvent::Match { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(matched.len(), 1);
        assert!(matched[0].ends_with("ok.txt"));
    }

    #[test]
    fn truncates_at_file_cap() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..(FILE_CAP + 10) {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), "needle\n").unwrap();
        }
        let events = collect(tmp.path(), "needle", true);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: true,
                ..
            })
        ));
        assert_eq!(events.len() - 1, FILE_CAP as usize);
    }

    #[test]
    fn truncates_at_total_match_cap() {
        // Each file holds fewer matches than the per-file cap, so the total cap
        // (not the per-file or file cap) is what stops the walk.
        let tmp = tempfile::tempdir().unwrap();
        let per_file = 100usize;
        let files = (TOTAL_MATCH_CAP as usize / per_file) + 5;
        let body = "needle\n".repeat(per_file);
        for i in 0..files {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), &body).unwrap();
        }
        let events = collect(tmp.path(), "needle", true);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: true,
                ..
            })
        ));
        let total: usize = events
            .iter()
            .filter_map(|e| match e {
                SearchEvent::Match { matches, .. } => Some(matches.len()),
                _ => None,
            })
            .sum();
        assert_eq!(total, TOTAL_MATCH_CAP as usize);
    }

    #[test]
    fn caps_matches_per_file() {
        // A single file with more matches than the per-file cap emits exactly the
        // cap and, since the total stays under the global cap, is not truncated.
        let tmp = tempfile::tempdir().unwrap();
        let body = "needle\n".repeat(PER_FILE_MATCH_CAP + 50);
        std::fs::write(tmp.path().join("a.txt"), &body).unwrap();
        let events = collect(tmp.path(), "needle", true);
        let SearchEvent::Match { matches, .. } = &events[0] else {
            panic!()
        };
        assert_eq!(matches.len(), PER_FILE_MATCH_CAP);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: false,
                file_count: 1
            })
        ));
    }

    #[test]
    fn wire_contract_serializes_with_type_tag_and_camel_case() {
        let m = SearchEvent::Match {
            path: "a.txt".into(),
            matches: vec![SearchMatch {
                line: 1,
                col: 0,
                preview: "hi".into(),
                ranges: None,
            }],
        };
        let d = SearchEvent::Done {
            truncated: true,
            file_count: 5000,
        };
        // Wire contract produced by the mandated attributes on the enum
        // (`#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "type")]`):
        //  - variant tags are camelCased: `Match` -> "match", `Done` -> "done"
        //  - `rename_all_fields` camelCases struct-variant fields, so
        //    `file_count` -> `fileCount` on the wire, matching the T9 TS
        //    contract. `SearchMatch` (a separate struct with its own attribute)
        //    stays camelCased independently.
        // T9/T18 must consume these exact keys.
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"type":"match","path":"a.txt","matches":[{"line":1,"col":0,"preview":"hi"}]}"#
        );
        assert_eq!(
            serde_json::to_string(&d).unwrap(),
            r#"{"type":"done","truncated":true,"fileCount":5000}"#
        );
    }

    #[test]
    fn columns_count_original_unicode_scalars() {
        for case_sensitive in [false, true] {
            for (line, col) in [("İİx", 2), ("😀éx", 2), ("中文 x", 3)] {
                let tmp = tempfile::tempdir().unwrap();
                std::fs::write(tmp.path().join("a.txt"), format!("{line}\n")).unwrap();
                let events = collect(tmp.path(), "x", case_sensitive);
                let SearchEvent::Match { matches, .. } = &events[0] else {
                    panic!("missing match")
                };
                assert_eq!((matches[0].line, matches[0].col), (1, col));
                assert_eq!(matches[0].preview, line);
            }
        }
    }

    #[test]
    fn preview_ranges_follow_the_matcher_in_utf16_offsets() {
        // Case folding the UI's toLowerCase cannot reproduce, a surrogate pair
        // before the hit, and leading whitespace trimmed from the preview.
        for (line, query, preview, ranges) in [
            ("ΣΑΣ x ς", "σ", "ΣΑΣ x ς", vec![[0, 1], [2, 3], [6, 7]]),
            ("😀s ſ", "s", "😀s ſ", vec![[2, 3], [4, 5]]),
            ("   ab ab", "b", "ab ab", vec![[1, 2], [4, 5]]),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(tmp.path().join("a.txt"), format!("{line}\n")).unwrap();
            let events = collect(tmp.path(), query, false);
            let SearchEvent::Match { matches, .. } = &events[0] else {
                panic!("missing match for {query:?}")
            };
            assert_eq!(matches[0].preview, preview);
            assert_eq!(matches[0].ranges.as_deref(), Some(ranges.as_slice()));
        }
        assert_eq!(
            serde_json::to_string(&SearchMatch {
                line: 1,
                col: 0,
                preview: "ab".into(),
                ranges: Some(vec![[1, 2]]),
            })
            .unwrap(),
            r#"{"line":1,"col":0,"preview":"ab","ranges":[[1,2]]}"#
        );
    }

    #[test]
    fn empty_query_finishes_without_matches_and_metacharacters_stay_literal() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a.b\naxb\n").unwrap();
        assert!(matches!(
            collect(tmp.path(), "", false).as_slice(),
            [SearchEvent::Done {
                truncated: false,
                file_count: 0
            }]
        ));
        for case_sensitive in [false, true] {
            let events = collect(tmp.path(), ".", case_sensitive);
            let SearchEvent::Match { matches, .. } = &events[0] else {
                panic!("missing match")
            };
            assert_eq!(matches.len(), 1);
            assert_eq!((matches[0].line, matches[0].col), (1, 1));
        }
    }

    #[test]
    fn searcher_resets_encoding_binary_detection_and_line_numbers_between_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut utf16 = vec![0xff, 0xfe];
        for unit in "ordinary\nneedle\n".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(tmp.path().join("a-utf16.txt"), utf16).unwrap();
        std::fs::write(tmp.path().join("b-binary.dat"), b"needle\0binary\n").unwrap();
        std::fs::write(tmp.path().join("c-utf8.txt"), "😀 needle\n").unwrap();
        std::fs::write(tmp.path().join("d-long.txt"), "ordinary\n".repeat(10_000)).unwrap();
        std::fs::write(tmp.path().join("e-last.txt"), "needle\n").unwrap();

        let events = collect(tmp.path(), "needle", true);
        let mut matches = events
            .iter()
            .filter_map(|event| match event {
                SearchEvent::Match { path, matches } => Some((
                    std::path::Path::new(path)
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    matches,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|(name, _)| *name);
        assert_eq!(matches.len(), 3);
        for ((name, found), (expected_name, line, col, range)) in matches.iter().zip([
            ("a-utf16.txt", 2, 0, [0, 6]),
            ("c-utf8.txt", 1, 2, [3, 9]),
            ("e-last.txt", 1, 0, [0, 6]),
        ]) {
            assert_eq!(*name, expected_name);
            assert_eq!(found.len(), 1);
            assert_eq!((found[0].line, found[0].col), (line, col));
            assert_eq!(found[0].ranges.as_deref(), Some([range].as_slice()));
        }
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: false,
                file_count: 3,
            })
        ));
    }

    #[test]
    fn stale_generation_stops_without_done() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "needle\n").unwrap();
        let gen = AtomicU64::new(2); // 已被新查詢超越
        let mut out = Vec::new();
        run_search(tmp.path(), "needle", true, 1, &gen, &mut |e| out.push(e));
        assert!(out.is_empty());
    }
}
