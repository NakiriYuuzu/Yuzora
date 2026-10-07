use crate::file_content::{
    analyze_byte_content, ByteContent, FILE_ANALYSIS_BYTES, FULL_FEATURE_MAX_BYTES,
};
use serde_json::{json, Value};

fn detect_line_ending(content: &str) -> &'static str {
    let bytes = content.as_bytes();
    let mut previous = 0;
    for (index, _) in content.match_indices('\r') {
        if bytes.get(index + 1) != Some(&b'\n') || content[previous..index].contains('\n') {
            return "mixed";
        }
        previous = index + 2;
    }
    if previous == 0 {
        "lf"
    } else if content[previous..].contains('\n') {
        "mixed"
    } else {
        "crlf"
    }
}

pub fn classify_bytes(bytes: &[u8]) -> Value {
    let size = bytes.len();
    match analyze_byte_content(&bytes[..size.min(FILE_ANALYSIS_BYTES)]) {
        ByteContent::Binary => json!({"kind":"binary", "size":size}),
        ByteContent::Utf16Le | ByteContent::Utf16Be => {
            let be = bytes.starts_with(&[0xfe, 0xff]);
            let codec = if be {
                encoding_rs::UTF_16BE
            } else {
                encoding_rs::UTF_16LE
            };
            let (text, _, _) = codec.decode(bytes);
            json!({"kind":"nonUtf8Readonly", "size":size, "content":text, "encoding":if be { "UTF-16BE" } else { "UTF-16LE" }})
        }
        ByteContent::Text => match std::str::from_utf8(bytes) {
            Err(_) => {
                json!({"kind":"nonUtf8Readonly", "size":size, "content":String::from_utf8_lossy(bytes), "encoding":"unknown"})
            }
            Ok(content) => {
                let line_ending = detect_line_ending(content);
                json!({"kind": if size as u64 > FULL_FEATURE_MAX_BYTES { "limited" } else { "full" }, "content":content, "size":size, "lineEnding":line_ending})
            }
        },
    }
}

#[cfg(test)]
#[path = "content/tests.rs"]
mod tests;
