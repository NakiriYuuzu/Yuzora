//! Shared Herdr framing, schema, and IPC ceilings.
//!
//! Every line-oriented Herdr lane (API, events, connector stdout/stderr) must
//! read through [`read_bounded_ndjson_line`] so a hostile peer cannot grow
//! allocations past 1 MiB before JSON parse. Parsed payloads are then checked
//! for complexity and serialized IPC size before they can reach the renderer.

use std::io::{BufRead, Read};

use serde::Serialize;

pub const MAX_NDJSON_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_AGENT_TEXT_BYTES: usize = 512 * 1024;
pub const MAX_IPC_BYTES: usize = MAX_NDJSON_LINE_BYTES + 16 * 1024;
pub const MAX_JSON_DEPTH: usize = 64;
pub const MAX_JSON_ARRAY_LEN: usize = 8192;
pub const MAX_JSON_OBJECT_KEYS: usize = 2048;
pub const MAX_LAYOUT_DEPTH: usize = 32;
pub const MAX_SESSION_COUNT: usize = 256;
pub const MAX_WORKSPACE_COUNT: usize = 512;
pub const MAX_TAB_COUNT: usize = 2048;
pub const MAX_PANE_COUNT: usize = 4096;
pub const MAX_AGENT_COUNT: usize = 4096;
pub const MAX_AGENT_MANIFEST_COUNT: usize = 256;
pub const MAX_WORKTREE_COUNT: usize = 512;
pub const MAX_STATE_LABELS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrProtocolError {
    LineTooLarge,
    UnterminatedOverLimit,
    InvalidUtf8,
    EmptyResponse,
    InvalidJson,
    ResponseTooLarge,
    TimedOut,
    TooComplex(&'static str),
}

impl HerdrProtocolError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LineTooLarge | Self::UnterminatedOverLimit | Self::ResponseTooLarge => "tooLarge",
            Self::InvalidUtf8 => "invalidUtf8",
            Self::EmptyResponse => "emptyResponse",
            Self::InvalidJson => "invalidJson",
            Self::TimedOut => "timeout",
            Self::TooComplex(_) => "tooComplex",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::LineTooLarge => "Herdr NDJSON line exceeds 1 MiB",
            Self::UnterminatedOverLimit => "Herdr NDJSON line is unterminated and exceeds 1 MiB",
            Self::InvalidUtf8 => "Herdr NDJSON is not UTF-8",
            Self::EmptyResponse => "Herdr NDJSON response is empty",
            Self::InvalidJson => "Herdr NDJSON is not valid JSON",
            Self::ResponseTooLarge => "Herdr response exceeds the IPC size limit",
            Self::TimedOut => "Herdr CLI timed out",
            Self::TooComplex(reason) => match *reason {
                "depth" => "Herdr JSON exceeds the nesting depth limit",
                "array" => "Herdr JSON array exceeds the item limit",
                "object" => "Herdr JSON object exceeds the key limit",
                "workspaces" => "Herdr snapshot workspace count exceeds the limit",
                "tabs" => "Herdr snapshot tab count exceeds the limit",
                "panes" => "Herdr snapshot pane count exceeds the limit",
                "agents" => "Herdr snapshot agent count exceeds the limit",
                "agent_manifests" => "Herdr agent manifest count exceeds the limit",
                "layouts" => "Herdr snapshot layout count exceeds the limit",
                "sessions" => "Herdr session count exceeds the limit",
                "worktrees" => "Herdr worktree count exceeds the limit",
                "state_labels" => "Herdr state_labels count exceeds the limit",
                "layout depth" => "Herdr layout exceeds the recursion limit",
                _ => "Herdr JSON exceeds complexity limits",
            },
        }
    }
}

impl std::fmt::Display for HerdrProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl From<HerdrProtocolError> for String {
    fn from(error: HerdrProtocolError) -> Self {
        error.to_string()
    }
}

#[derive(Debug)]
pub enum BoundedNdjsonReadError {
    Io(std::io::Error),
    Protocol(HerdrProtocolError),
}

impl std::fmt::Display for BoundedNdjsonReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Protocol(error) => write!(f, "{error}"),
        }
    }
}

impl From<std::io::Error> for BoundedNdjsonReadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<HerdrProtocolError> for BoundedNdjsonReadError {
    fn from(error: HerdrProtocolError) -> Self {
        Self::Protocol(error)
    }
}

/// Read one NDJSON line with a hard 1 MiB ceiling before JSON parse.
///
/// Returns `Ok(0)` on clean EOF. Over-limit content, an over-limit
/// unterminated read, and invalid UTF-8 are typed protocol errors.
pub fn read_bounded_ndjson_line<R: BufRead>(
    reader: &mut R,
    output: &mut String,
) -> Result<usize, BoundedNdjsonReadError> {
    let mut bytes = Vec::new();
    // Read one extra byte past the cap so a terminated MAX-byte line is
    // accepted, while MAX+1 content (with or without a newline) is rejected.
    let mut limited = reader.take((MAX_NDJSON_LINE_BYTES + 2) as u64);
    let read = match limited.read_until(b'\n', &mut bytes) {
        Ok(read) => read,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) && bytes.len() > MAX_NDJSON_LINE_BYTES =>
        {
            return Err(BoundedNdjsonReadError::Protocol(
                HerdrProtocolError::UnterminatedOverLimit,
            ));
        }
        Err(error) => return Err(BoundedNdjsonReadError::Io(error)),
    };
    if read == 0 {
        output.clear();
        return Ok(0);
    }
    let terminated = bytes.last() == Some(&b'\n');
    let content_len = if terminated {
        bytes.len().saturating_sub(1)
    } else {
        bytes.len()
    };
    if content_len > MAX_NDJSON_LINE_BYTES {
        return Err(BoundedNdjsonReadError::Protocol(if terminated {
            HerdrProtocolError::LineTooLarge
        } else {
            HerdrProtocolError::UnterminatedOverLimit
        }));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| BoundedNdjsonReadError::Protocol(HerdrProtocolError::InvalidUtf8))?;
    output.clear();
    output.push_str(&text);
    Ok(read)
}

/// Read a pipe until EOF or the byte ceiling. Oversized content is a typed
/// `tooLarge` error and must not be parsed as JSON.
pub fn read_bounded_bytes<R: Read>(
    reader: &mut R,
    max: usize,
) -> Result<Vec<u8>, BoundedNdjsonReadError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(buf),
            Ok(n) => {
                if buf.len().saturating_add(n) > max {
                    return Err(BoundedNdjsonReadError::Protocol(
                        HerdrProtocolError::ResponseTooLarge,
                    ));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(BoundedNdjsonReadError::Io(error)),
        }
    }
}

/// Strict-UTF-8 JSON parse for a completed Herdr CLI stdout buffer.
pub fn parse_herdr_cli_stdout(stdout: &[u8]) -> Result<serde_json::Value, HerdrProtocolError> {
    if stdout.len() > MAX_NDJSON_LINE_BYTES {
        return Err(HerdrProtocolError::ResponseTooLarge);
    }
    let text = std::str::from_utf8(stdout).map_err(|_| HerdrProtocolError::InvalidUtf8)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(HerdrProtocolError::EmptyResponse);
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| HerdrProtocolError::InvalidJson)?;
    validate_json_complexity(&value)?;
    Ok(value)
}

pub fn validate_json_complexity(value: &serde_json::Value) -> Result<(), HerdrProtocolError> {
    walk_json_complexity(value, 0)
}

fn walk_json_complexity(value: &serde_json::Value, depth: usize) -> Result<(), HerdrProtocolError> {
    if depth > MAX_JSON_DEPTH {
        return Err(HerdrProtocolError::TooComplex("depth"));
    }
    match value {
        serde_json::Value::Array(items) => {
            if items.len() > MAX_JSON_ARRAY_LEN {
                return Err(HerdrProtocolError::TooComplex("array"));
            }
            for item in items {
                walk_json_complexity(item, depth + 1)?;
            }
        }
        serde_json::Value::Object(map) => {
            if map.len() > MAX_JSON_OBJECT_KEYS {
                return Err(HerdrProtocolError::TooComplex("object"));
            }
            for child in map.values() {
                walk_json_complexity(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_snapshot_counts(snapshot: &serde_json::Value) -> Result<(), HerdrProtocolError> {
    let len = |key: &str| {
        snapshot
            .get(key)
            .and_then(|value| value.as_array())
            .map(|items| items.len())
            .unwrap_or(0)
    };
    if len("workspaces") > MAX_WORKSPACE_COUNT {
        return Err(HerdrProtocolError::TooComplex("workspaces"));
    }
    if len("tabs") > MAX_TAB_COUNT {
        return Err(HerdrProtocolError::TooComplex("tabs"));
    }
    if len("panes") > MAX_PANE_COUNT {
        return Err(HerdrProtocolError::TooComplex("panes"));
    }
    if len("agents") > MAX_AGENT_COUNT {
        return Err(HerdrProtocolError::TooComplex("agents"));
    }
    if len("layouts") > MAX_TAB_COUNT {
        return Err(HerdrProtocolError::TooComplex("layouts"));
    }
    Ok(())
}

/// Cap agent text at 512 KiB. Oversized content is not delivered in full;
/// the returned flag is the explicit `tooLarge` wire signal.
pub fn bound_agent_text(text: String) -> (String, bool) {
    if text.len() <= MAX_AGENT_TEXT_BYTES {
        return (text, false);
    }
    (truncate_utf8(&text, MAX_AGENT_TEXT_BYTES), true)
}

pub fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

pub fn ensure_ipc_bound<T: Serialize>(value: &T) -> Result<(), HerdrProtocolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| HerdrProtocolError::InvalidJson)?;
    if bytes.len() > MAX_IPC_BYTES {
        return Err(HerdrProtocolError::ResponseTooLarge);
    }
    Ok(())
}

/// Drop a nested JSON document that exceeds complexity or IPC ceilings.
pub fn bound_optional_json(value: serde_json::Value) -> Option<serde_json::Value> {
    if validate_json_complexity(&value).is_ok() && ensure_ipc_bound(&value).is_ok() {
        Some(value)
    } else {
        None
    }
}

pub fn bounded_ipc<T: Serialize>(value: T) -> Result<T, String> {
    ensure_ipc_bound(&value).map_err(String::from)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_line(bytes: &[u8]) -> Result<(usize, String), BoundedNdjsonReadError> {
        let mut cursor = Cursor::new(bytes);
        let mut output = String::new();
        let read = read_bounded_ndjson_line(&mut cursor, &mut output)?;
        Ok((read, output))
    }

    fn nest_arrays(depth: usize) -> serde_json::Value {
        let mut value = serde_json::json!(1);
        for _ in 0..depth {
            value = serde_json::json!([value]);
        }
        value
    }

    #[test]
    fn oversized_unterminated_line_is_rejected_before_parse() {
        let bytes = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
        match read_line(&bytes) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::UnterminatedOverLimit)) => {}
            other => panic!("expected unterminated over-limit, got {other:?}"),
        }
    }

    #[test]
    fn oversized_terminated_line_is_rejected_before_parse() {
        let mut bytes = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
        bytes.push(b'\n');
        match read_line(&bytes) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::LineTooLarge)) => {}
            other => panic!("expected line too large, got {other:?}"),
        }
    }

    #[test]
    fn line_at_byte_cap_is_accepted() {
        let mut bytes = vec![b'x'; MAX_NDJSON_LINE_BYTES];
        bytes.push(b'\n');
        let (read, output) = read_line(&bytes).expect("exact cap must pass");
        assert_eq!(read, MAX_NDJSON_LINE_BYTES + 1);
        assert_eq!(output.len(), MAX_NDJSON_LINE_BYTES + 1);
    }

    #[test]
    fn invalid_utf8_is_rejected_before_parse() {
        match read_line(&[0xff, 0xfe, b'\n']) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::InvalidUtf8)) => {}
            other => panic!("expected invalid UTF-8, got {other:?}"),
        }
    }

    #[test]
    fn empty_eof_is_zero_not_a_line() {
        let (read, output) = read_line(&[]).expect("eof");
        assert_eq!(read, 0);
        assert!(output.is_empty());
    }

    #[test]
    fn json_depth_and_array_limits_reject_hostile_payloads() {
        assert!(validate_json_complexity(&nest_arrays(MAX_JSON_DEPTH)).is_ok());
        assert_eq!(
            validate_json_complexity(&nest_arrays(MAX_JSON_DEPTH + 1)),
            Err(HerdrProtocolError::TooComplex("depth"))
        );
        let just_below = vec![serde_json::json!(1); MAX_JSON_ARRAY_LEN];
        let just_above = vec![serde_json::json!(1); MAX_JSON_ARRAY_LEN + 1];
        assert!(validate_json_complexity(&serde_json::Value::Array(just_below)).is_ok());
        assert_eq!(
            validate_json_complexity(&serde_json::Value::Array(just_above)),
            Err(HerdrProtocolError::TooComplex("array"))
        );
    }

    #[test]
    fn snapshot_count_ceiling_is_exclusive_of_the_limit_plus_one() {
        let panes = vec![serde_json::json!({"pane_id": "p"}); MAX_PANE_COUNT];
        let ok = serde_json::json!({ "panes": panes });
        assert!(validate_snapshot_counts(&ok).is_ok());
        let mut over = vec![serde_json::json!({"pane_id": "p"}); MAX_PANE_COUNT];
        over.push(serde_json::json!({"pane_id": "overflow"}));
        let err = validate_snapshot_counts(&serde_json::json!({ "panes": over })).unwrap_err();
        assert_eq!(err, HerdrProtocolError::TooComplex("panes"));
    }

    #[test]
    fn agent_text_just_below_and_above_512_kib() {
        let exact = "a".repeat(MAX_AGENT_TEXT_BYTES);
        let (kept, too_large) = bound_agent_text(exact.clone());
        assert_eq!(kept, exact);
        assert!(!too_large);

        let over = "a".repeat(MAX_AGENT_TEXT_BYTES + 1);
        let (capped, too_large) = bound_agent_text(over);
        assert_eq!(capped.len(), MAX_AGENT_TEXT_BYTES);
        assert!(too_large);
        assert!(!capped.contains('\u{fffd}'));
    }

    #[test]
    fn ipc_size_just_below_and_above_limit() {
        let below = "x".repeat(MAX_IPC_BYTES - 2);
        assert!(ensure_ipc_bound(&below).is_ok());
        let above = "x".repeat(MAX_IPC_BYTES - 1);
        assert_eq!(
            ensure_ipc_bound(&above),
            Err(HerdrProtocolError::ResponseTooLarge)
        );
    }

    #[test]
    fn cli_stdout_oversized_is_too_large() {
        let bytes = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
        assert_eq!(
            parse_herdr_cli_stdout(&bytes),
            Err(HerdrProtocolError::ResponseTooLarge)
        );
    }

    #[test]
    fn cli_stdout_invalid_utf8_is_rejected() {
        assert_eq!(
            parse_herdr_cli_stdout(&[0xff, 0xfe]),
            Err(HerdrProtocolError::InvalidUtf8)
        );
    }

    #[test]
    fn cli_stdout_normal_json_is_parsed() {
        let value = parse_herdr_cli_stdout(br#"{"ok":true}"#).expect("json");
        assert_eq!(value["ok"], true);
    }

    #[test]
    fn bounded_byte_reader_stops_past_the_cap() {
        let bytes = vec![b'a'; 16];
        match read_bounded_bytes(&mut Cursor::new(&bytes), 8) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::ResponseTooLarge)) => {}
            other => panic!("expected tooLarge, got {other:?}"),
        }
        let kept = read_bounded_bytes(&mut Cursor::new(&bytes), 16).expect("exact cap");
        assert_eq!(kept, bytes);
    }
}
