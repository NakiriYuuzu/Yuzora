use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct SearchState(pub std::sync::Arc<AtomicU64>);

const FILE_CAP: u32 = 5000;
const PREVIEW_LEN: usize = 200;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub line: u32,
    pub col: u32,
    pub preview: String,
}

#[derive(Clone, serde::Serialize)]
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

fn find_col(line: &str, query: &str, case_sensitive: bool) -> Option<u32> {
    // Case-insensitive matching must count the column on the *same* lowercased
    // string the byte offset came from: `to_lowercase()` can change byte lengths
    // (e.g. 'İ' U+0130 → "i\u{0307}"), so slicing the original `line` by an offset
    // found in its lowercased form can land mid-char and panic.
    if case_sensitive {
        let byte = line.find(query)?;
        Some(line[..byte].chars().count() as u32)
    } else {
        let lower = line.to_lowercase();
        let byte = lower.find(&query.to_lowercase())?;
        Some(lower[..byte].chars().count() as u32)
    }
}

fn make_preview(line: &str) -> String {
    let trimmed = line.trim();
    trimmed.chars().take(PREVIEW_LEN).collect()
}

/// Collects matches for a single file. Stops the search and discards nothing
/// extra when binary data is detected — returning `false` from `binary_data`
/// makes the searcher quit before matching the truncated binary line.
struct MatchCollector<'a> {
    query: &'a str,
    case_sensitive: bool,
    matches: Vec<SearchMatch>,
}

impl Sink for MatchCollector<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(mat.bytes());
        if let Some(col) = find_col(&line, self.query, self.case_sensitive) {
            self.matches.push(SearchMatch {
                line: mat.line_number().unwrap_or(0) as u32,
                col,
                preview: make_preview(&line),
            });
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

    for entry in WalkBuilder::new(root).require_git(false).build() {
        if gen_source.load(Ordering::Relaxed) != generation {
            return;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .build();
        let mut collector = MatchCollector {
            query,
            case_sensitive,
            matches: Vec::new(),
        };
        let _ = searcher.search_path(&matcher, entry.path(), &mut collector);

        if collector.matches.is_empty() {
            continue;
        }

        emit(SearchEvent::Match {
            path: entry.path().to_string_lossy().into_owned(),
            matches: collector.matches,
        });
        file_count += 1;

        if file_count >= FILE_CAP {
            emit(SearchEvent::Done {
                truncated: true,
                file_count,
            });
            return;
        }
    }

    emit(SearchEvent::Done {
        truncated: false,
        file_count,
    });
}

#[tauri::command]
pub fn search_workspace(
    state: tauri::State<SearchState>,
    root: String,
    query: String,
    case_sensitive: bool,
    on_event: tauri::ipc::Channel<SearchEvent>,
) -> Result<(), String> {
    let generation = state.0.fetch_add(1, Ordering::SeqCst) + 1;
    let gen_source = state.0.clone();
    std::thread::spawn(move || {
        run_search(
            Path::new(&root),
            &query,
            case_sensitive,
            generation,
            &gen_source,
            &mut |event| {
                let _ = on_event.send(event);
            },
        );
    });
    Ok(())
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
    #[ignore = "creates 5010 files; run with --ignored"]
    fn truncates_at_file_cap() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5010 {
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
        assert_eq!(events.len() - 1, 5000);
    }

    #[test]
    fn wire_contract_serializes_with_type_tag_and_camel_case() {
        let m = SearchEvent::Match {
            path: "a.txt".into(),
            matches: vec![SearchMatch {
                line: 1,
                col: 0,
                preview: "hi".into(),
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
    fn find_col_handles_variable_length_lowercase() {
        // U+0130 'İ' lowercases to two chars (i + combining dot above), so a byte
        // offset taken in the lowercased string can exceed the original line's byte
        // length. Slicing the original line by that offset used to panic; col is now
        // computed on the same lowercased string it was found in.
        // lowercase("İİx") == "i\u{0307}i\u{0307}x"; 'x' is char index 4.
        assert_eq!(find_col("İİx", "x", false), Some(4));
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
