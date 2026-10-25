use encoding_rs_io::DecodeReaderBytesBuilder;
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

fn find_col(line: &[u8], matcher: &RegexMatcher) -> std::io::Result<Option<u32>> {
    Ok(matcher.find(line)?.map(|found| {
        String::from_utf8_lossy(&line[..found.start()])
            .chars()
            .count() as u32
    }))
}

fn make_preview(line: &str) -> String {
    let trimmed = line.trim();
    trimmed.chars().take(PREVIEW_LEN).collect()
}

/// Match the editor's LF, CRLF and CR lines after BOM decoding. Normalizing raw
/// bytes would corrupt UTF-16 code units that contain a CR byte.
struct LogicalNewlineReader<R> {
    inner: R,
    skip_lf: bool,
}

impl<R> LogicalNewlineReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            skip_lf: false,
        }
    }
}

impl<R: Read> Read for LogicalNewlineReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let len = self.inner.read(buf)?;
            if len == 0 {
                return Ok(0);
            }
            let start = usize::from(self.skip_lf && buf[0] == b'\n');
            self.skip_lf = false;
            if start == len {
                // A skipped LF is not EOF; the next read may also fail.
                continue;
            }
            if !buf[start..len].contains(&b'\r') {
                if start != 0 {
                    buf.copy_within(start..len, 0);
                }
                return Ok(len - start);
            }
            let mut read = start;
            let mut written = 0;
            while read < len {
                let byte = buf[read];
                read += 1;
                if byte == b'\r' {
                    buf[written] = b'\n';
                    if read == len {
                        self.skip_lf = true;
                    } else if buf[read] == b'\n' {
                        read += 1;
                    }
                } else {
                    buf[written] = byte;
                }
                written += 1;
            }
            return Ok(written);
        }
    }
}

fn logical_line_reader<'a, R: Read + 'a>(mut raw: R) -> std::io::Result<impl Read + 'a> {
    let mut prefix = [0; 3];
    let mut filled = 0;
    while filled < prefix.len() {
        match raw.read(&mut prefix[filled..]) {
            Ok(0) => break,
            Ok(len) => filled += len,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    // The UTF-16 decoder removes its own BOM. Keep it for the decoder so a
    // second, literal U+FEFF is not removed too. Match encoding_rs_io's 3-byte
    // sniff requirement; a two-byte BOM-only file is stripped directly below.
    let utf16 = filled >= 3 && matches!(&prefix[..2], [0xff, 0xfe] | [0xfe, 0xff]);
    let skip = if filled == 3 && prefix == [0xef, 0xbb, 0xbf] {
        3
    } else if filled == 2 && matches!(&prefix[..2], [0xff, 0xfe] | [0xfe, 0xff]) {
        2
    } else {
        0
    };
    let mut head = std::io::Cursor::new(prefix);
    head.set_position(skip as u64);
    let bytes = head.take((filled - skip) as u64).chain(raw);
    let decoded: Box<dyn Read + 'a> = if utf16 {
        let decoder = DecodeReaderBytesBuilder::new()
            .encoding(None)
            .utf8_passthru(true)
            .strip_bom(false)
            .bom_override(true)
            .bom_sniffing(true)
            .build(bytes);
        // The decoder's tiny-buffer EOF path can drop pending replacement bytes.
        // Give it room for one UTF-8 codepoint even when our caller reads one byte.
        Box::new(std::io::BufReader::with_capacity(4, decoder))
    } else {
        // Preserve invalid UTF-8 bytes for matching, without a decoder buffer.
        Box::new(bytes)
    };
    Ok(LogicalNewlineReader::new(decoded))
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
    probe_overflow: bool,
    truncated: bool,
}

impl Sink for MatchCollector<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(mat.bytes());
        if let Some(col) = find_col(mat.bytes(), self.matcher)? {
            if self.matches.len() >= self.budget {
                self.truncated = true;
                return Ok(false);
            }
            self.matches.push(SearchMatch {
                line: mat.line_number().unwrap_or(0) as u32,
                col,
                preview: make_preview(&line),
            });
            // Global limits already imply truncation; only a per-file ceiling needs lookahead.
            if !self.probe_overflow && self.matches.len() >= self.budget {
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
        let mut searcher = SearcherBuilder::new()
            // The outer decoder strips only the file BOM. A second U+FEFF is text.
            .bom_sniffing(false)
            .binary_detection(BinaryDetection::quit(0))
            .build();
        // Cap this file at the per-file ceiling but never past the remaining
        // global budget, so the total never overshoots TOTAL_MATCH_CAP.
        let remaining = (TOTAL_MATCH_CAP - total_matches) as usize;
        let budget = remaining.min(PER_FILE_MATCH_CAP);
        let mut collector = MatchCollector {
            matcher: &matcher,
            matches: Vec::new(),
            budget,
            probe_overflow: remaining > PER_FILE_MATCH_CAP && file_count + 1 < FILE_CAP,
            truncated: false,
        };
        let _ = logical_line_reader(file.take(crate::protocol::MAX_FILE_BYTES))
            .and_then(|reader| searcher.search_reader(&matcher, reader, &mut collector));
        if gen_source.load(Ordering::Relaxed) != generation {
            return;
        }

        incomplete |= collector.truncated;
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

    fn assert_single_match(body: &[u8], query: &str, case_sensitive: bool, col: u32) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("unicode.txt"), body).unwrap();
        let events = collect(tmp.path(), query, case_sensitive);
        assert_eq!(events.len(), 2, "{events:?}");
        let SearchEvent::Match { matches, .. } = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(matches.len(), 1);
        assert_eq!((matches[0].line, matches[0].col), (1, col));
        let preview: String = String::from_utf8_lossy(body)
            .trim()
            .chars()
            .take(PREVIEW_LEN)
            .collect();
        assert_eq!(matches[0].preview, preview);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: false,
                file_count: 1
            })
        ));
    }

    fn assert_logical_lines(body: &[u8], expected: &[(u32, u32)]) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("lines.txt"), body).unwrap();
        let events = collect(tmp.path(), "needle", true);
        let found: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SearchEvent::Match { matches, .. } => Some(matches),
                _ => None,
            })
            .flatten()
            .map(|m| (m.line, m.col))
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn bare_cr_lines_match_editor_logical_line_numbers() {
        assert_logical_lines(b"header\rneedle\n", &[(2, 0)]);
        assert_logical_lines(b"needle\rneedle", &[(1, 0), (2, 0)]);
    }

    #[test]
    fn mixed_logical_lines_preserve_empty_lines_and_final_records() {
        assert_logical_lines(
            b"\rheader\r\r\nneedle\r\nneedle\nneedle\r",
            &[(4, 0), (5, 0), (6, 0)],
        );
        assert_logical_lines(b"needle\r\n\rneedle", &[(1, 0), (3, 0)]);
    }

    #[test]
    fn utf8_bom_logical_lines_and_literal_second_bom_are_preserved() {
        assert_logical_lines(
            "\u{feff}\u{feff}needle\rneedle".as_bytes(),
            &[(1, 1), (2, 0)],
        );
    }

    #[test]
    fn utf16_bom_decoding_precedes_logical_line_normalization() {
        for little_endian in [true, false] {
            let mut bytes = if little_endian {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in "č中😀needle\r\nneedle\rneedle".encode_utf16() {
                bytes.extend(if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_logical_lines(&bytes, &[(1, 3), (2, 0), (3, 0)]);
        }
    }

    #[test]
    fn ordinary_lf_and_crlf_line_numbers_remain() {
        assert_logical_lines(b"header\nneedle\nneedle", &[(2, 0), (3, 0)]);
        assert_logical_lines(b"header\r\nneedle\r\nneedle", &[(2, 0), (3, 0)]);
    }

    struct Chunked<'a> {
        bytes: &'a [u8],
        chunk: usize,
        fail_at_end: bool,
        calls: usize,
    }

    impl Read for Chunked<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.bytes.is_empty() && self.fail_at_end {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            let len = self.bytes.len().min(self.chunk).min(buf.len());
            buf[..len].copy_from_slice(&self.bytes[..len]);
            self.bytes = &self.bytes[len..];
            Ok(len)
        }
    }

    #[test]
    fn logical_newline_reader_handles_split_pairs_without_false_eof() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"a\r\nb", b"a\nb"),
            (b"\r\r\n", b"\n\n"),
            (b"\r\n\r\n\r", b"\n\n\n"),
            (b"\xff\r\0\nx", b"\xff\n\0\nx"),
            (b"\n\n", b"\n\n"),
        ];
        for &(input, expected) in cases {
            for chunk in 1..=3 {
                for capacity in [1, 2, 16] {
                    let mut reader = LogicalNewlineReader::new(Chunked {
                        bytes: input,
                        chunk,
                        fail_at_end: false,
                        calls: 0,
                    });
                    assert_eq!(reader.read(&mut []).unwrap(), 0);
                    assert_eq!(reader.inner.calls, 0);
                    let mut output = Vec::new();
                    let mut buf = vec![0; capacity];
                    loop {
                        let len = reader.read(&mut buf).unwrap();
                        if len == 0 {
                            break;
                        }
                        output.extend_from_slice(&buf[..len]);
                        assert_eq!(reader.read(&mut []).unwrap(), 0);
                    }
                    assert_eq!(output, expected, "chunk={chunk}, capacity={capacity}");
                }
            }
        }
    }

    #[test]
    fn logical_newline_reader_propagates_error_after_skipped_lf() {
        let mut reader = LogicalNewlineReader::new(Chunked {
            bytes: b"\r\n",
            chunk: 1,
            fail_at_end: true,
            calls: 0,
        });
        let mut buf = [0];
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(buf, [b'\n']);
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        assert_eq!(reader.inner.calls, 1);
        assert_eq!(
            reader.read(&mut buf).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(reader.inner.calls, 3);
    }

    #[test]
    fn utf16_second_bom_remains_text_after_decoding() {
        for little_endian in [true, false] {
            let mut bytes = if little_endian {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in "\u{feff}č中😀needle\rneedle".encode_utf16() {
                bytes.extend(if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_logical_lines(&bytes, &[(1, 4), (2, 0)]);
        }
    }

    #[test]
    fn decoded_logical_lines_preserve_short_boms_and_invalid_utf8() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b""),
            (b"\xff", b"\xff"),
            (b"\xef\xbb", b"\xef\xbb"),
            (b"\xff\xfe", b""),
            (b"\xfe\xff", b""),
            (b"\xff\xfeA", "�".as_bytes()),
            (b"\xfe\xff\0", "�".as_bytes()),
            (b"\xef\xbb\xbf\xff\r\n", b"\xff\n"),
            (b"\xff\xfe\xff\xfe", "\u{feff}".as_bytes()),
            (b"\xfe\xff\xfe\xff", "\u{feff}".as_bytes()),
            (b"\xff\xfe\0\0", b"\0"),
            (b"\0\0\xfe\xff", b"\0\0\xfe\xff"),
            (b"\xff\xfe\r\x01\r\0\n\0", "č\n".as_bytes()),
            (b"\xfe\xff\x01\r\0\r\0\n", "č\n".as_bytes()),
        ];
        for &(input, expected) in cases {
            for chunk in 1..=3 {
                for capacity in [1, 2, 16] {
                    let mut reader = logical_line_reader(Chunked {
                        bytes: input,
                        chunk,
                        fail_at_end: false,
                        calls: 0,
                    })
                    .unwrap();
                    let mut buf = vec![0; capacity];
                    let mut output = Vec::new();
                    loop {
                        let len = reader.read(&mut buf).unwrap();
                        if len == 0 {
                            break;
                        }
                        output.extend_from_slice(&buf[..len]);
                    }
                    assert_eq!(
                        output, expected,
                        "input={input:?}, chunk={chunk}, capacity={capacity}"
                    );
                }
            }
        }
    }

    #[test]
    fn decoded_logical_lines_obey_raw_limit_at_encoding_boundaries() {
        for (input, limit, expected) in [
            (b"\r\nneedle".as_slice(), 2, b"\n".as_slice()),
            (b"\xef\xbb\xbfneedle", 2, b"\xef\xbb"),
            (b"\xff\xfeA\0", 2, b""),
            (b"\xff\xfeA\0", 3, "�".as_bytes()),
            (b"\xff\xfeA\0", 4, b"A"),
        ] {
            let mut reader = logical_line_reader(input.take(limit)).unwrap();
            let mut output = Vec::new();
            reader.read_to_end(&mut output).unwrap();
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn logical_line_prefix_retries_interrupted_and_propagates_other_errors() {
        struct InterruptOnce<'a> {
            inner: Chunked<'a>,
            interrupted: bool,
        }
        impl Read for InterruptOnce<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.inner.calls == 1 && !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                self.inner.read(buf)
            }
        }
        let input = InterruptOnce {
            inner: Chunked {
                bytes: b"\xff\xfeA\0",
                chunk: 1,
                fail_at_end: false,
                calls: 0,
            },
            interrupted: false,
        };
        let mut reader = logical_line_reader(input).unwrap();
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"A");
        let error = logical_line_reader(Chunked {
            bytes: b"\xff\xfe",
            chunk: 1,
            fail_at_end: true,
            calls: 0,
        })
        .err()
        .unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn logical_lines_keep_per_file_limits_and_binary_detection() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "needle\r".repeat(501)).unwrap();
        let events = collect(tmp.path(), "needle", true);
        let SearchEvent::Match { matches, .. } = &events[0] else {
            panic!("missing matches")
        };
        assert_eq!(matches.len(), 500);
        assert_eq!(matches.last().unwrap().line, 500);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: true,
                file_count: 1
            })
        ));
        std::fs::write(tmp.path().join("a.txt"), b"\0\rneedle").unwrap();
        let events = collect(tmp.path(), "needle", true);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SearchEvent::Done { file_count: 0, .. }));
    }

    #[test]
    fn logical_lines_keep_raw_file_size_limit() {
        let limit = crate::protocol::MAX_FILE_BYTES as usize;
        let mut body = b"\r\n".repeat((limit - 6) / 2);
        body.extend_from_slice(b"needle");
        assert_eq!(body.len(), limit);
        assert_logical_lines(&body, &[(((limit - 6) / 2 + 1) as u32, 0)]);
        body.push(b'\r');
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("oversized.txt"), body).unwrap();
        let events = collect(tmp.path(), "needle", true);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            SearchEvent::Done {
                truncated: true,
                file_count: 0
            }
        ));
    }

    #[test]
    fn unicode_long_s_matches_the_searchers_casefold() {
        assert_single_match("ſs\n".as_bytes(), "ss", false, 0);
        assert_single_match("😀中İİſs\n".as_bytes(), "ss", false, 4);
    }

    #[test]
    fn unicode_final_sigma_matches_the_searchers_casefold() {
        assert_single_match("ς\n".as_bytes(), "σ", false, 0);
    }

    #[test]
    fn unicode_columns_count_original_characters() {
        assert_single_match("İİx\n".as_bytes(), "x", false, 2);
        assert_single_match("😀中İİX\n".as_bytes(), "x", false, 4);
    }

    #[test]
    fn case_sensitive_and_insensitive_search_keep_their_first_actual_match() {
        assert_single_match("ſs ss\n".as_bytes(), "ss", true, 3);
        assert_single_match("ſs ss\n".as_bytes(), "ss", false, 0);
    }

    #[test]
    fn matching_uses_raw_bytes_before_lossy_preview_decoding() {
        assert_single_match(b"\xff \xef\xbf\xbd\n", "�", true, 2);
    }

    #[test]
    fn original_preview_and_fixed_string_semantics_remain() {
        assert_single_match(b"  A Needle b  \n", "needle", false, 4);
        assert_single_match(b"axb a.b\n", "a.b", true, 4);
        let long = format!("{} Needle\n", "X".repeat(PREVIEW_LEN + 10));
        assert_single_match(long.as_bytes(), "needle", false, (PREVIEW_LEN + 11) as u32);
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("dotted.txt"), "İ\n").unwrap();
        assert_eq!(collect(tmp.path(), "i", false).len(), 1);
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

    fn assert_file_truncation(body: &str, expected_matches: usize, truncated: bool) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("matches.txt"), body).unwrap();
        let events = collect(tmp.path(), "needle", true);
        let SearchEvent::Match { matches, .. } = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(matches.len(), expected_matches);
        assert!(
            matches!(events.last(), Some(SearchEvent::Done { truncated: actual, file_count: 1 }) if *actual == truncated),
            "{events:?}"
        );
    }

    #[test]
    fn per_file_truncation_distinguishes_499_500_and_501_lines() {
        for count in [499, 500, 501] {
            assert_file_truncation(
                &"needle\n".repeat(count),
                count.min(PER_FILE_MATCH_CAP),
                count > PER_FILE_MATCH_CAP,
            );
        }
    }

    #[test]
    fn per_file_truncation_looks_past_nonmatching_tail_for_an_extra_hit() {
        let body = "needle\n".repeat(500) + &"ordinary\n".repeat(4096);
        assert_file_truncation(&body, 500, false);
        assert_file_truncation(&(body + "needle\n"), 500, true);
    }

    #[test]
    fn per_file_truncation_counts_matching_lines_not_occurrences() {
        assert_file_truncation(&"needle ".repeat(501), 1, false);
    }

    #[test]
    fn per_file_truncation_is_aggregated_without_dropping_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("overflow.txt"), "needle\n".repeat(501)).unwrap();
        std::fs::write(tmp.path().join("complete.txt"), "needle\n").unwrap();
        let events = collect(tmp.path(), "needle", true);
        let mut counts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SearchEvent::Match { matches, .. } => Some(matches.len()),
                _ => None,
            })
            .collect();
        counts.sort();
        assert_eq!(counts, [1, 500]);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: true,
                file_count: 2
            })
        ));
    }

    #[test]
    fn multiple_exact_per_file_limits_are_complete_below_global_cap() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(tmp.path().join(name), "needle\n".repeat(500)).unwrap();
        }
        let events = collect(tmp.path(), "needle", true);
        assert!(matches!(
            events.last(),
            Some(SearchEvent::Done {
                truncated: false,
                file_count: 3
            })
        ));
        let count: usize = events
            .iter()
            .filter_map(|event| match event {
                SearchEvent::Match { matches, .. } => Some(matches.len()),
                _ => None,
            })
            .sum();
        assert_eq!(count, 1500);
    }

    #[test]
    fn caps_matches_per_file() {
        // A single file with more matches than the per-file cap emits exactly the
        // cap and reports that this file has additional matching lines.
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
                truncated: true,
                file_count: 1
            })
        ));
    }

    #[test]
    fn overflow_probe_reads_only_the_tail_needed_to_decide_completeness() {
        use std::cell::Cell;
        use std::rc::Rc;
        struct Counted<'a> {
            bytes: &'a [u8],
            read: Rc<Cell<usize>>,
        }
        impl Read for Counted<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let count = self.bytes.len().min(buf.len());
                buf[..count].copy_from_slice(&self.bytes[..count]);
                self.bytes = &self.bytes[count..];
                self.read.set(self.read.get() + count);
                Ok(count)
            }
        }
        let matcher = RegexMatcherBuilder::new()
            .fixed_strings(true)
            .build("needle")
            .unwrap();
        let tail = "ordinary\n".repeat(128 * 1024);
        for (lines, probe_overflow) in [(500, true), (501, true), (501, false)] {
            let body = "needle\n".repeat(lines) + &tail;
            let read = Rc::new(Cell::new(0));
            let reader = Counted {
                bytes: body.as_bytes(),
                read: read.clone(),
            };
            let mut collector = MatchCollector {
                matcher: &matcher,
                matches: Vec::new(),
                budget: 500,
                probe_overflow,
                truncated: false,
            };
            SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .build()
                .search_reader(
                    &matcher,
                    reader.take(crate::protocol::MAX_FILE_BYTES),
                    &mut collector,
                )
                .unwrap();
            assert_eq!(collector.matches.len(), 500);
            assert_eq!(collector.truncated, lines > 500 && probe_overflow);
            if lines == 500 {
                assert_eq!(
                    read.get(),
                    body.len(),
                    "exactly500 requires checking the nonmatching tail"
                );
            } else {
                assert!(
                    read.get() < body.len(),
                    "stop at the known limit; allow searcher buffering"
                );
            }
            assert!(read.get() as u64 <= crate::protocol::MAX_FILE_BYTES);
        }
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
    fn find_col_counts_original_characters_without_lowercase_offset_slicing() {
        let matcher = RegexMatcherBuilder::new()
            .fixed_strings(true)
            .case_insensitive(true)
            .build("x")
            .unwrap();
        // Lowercasing İ expands it, but the original column of x is still 2.
        assert_eq!(find_col("İİx".as_bytes(), &matcher).unwrap(), Some(2));
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
