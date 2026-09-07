use crate::file_content::{
    analyze_byte_content, ByteContent, FILE_ANALYSIS_BYTES, FULL_FEATURE_MAX_BYTES,
};
use serde_json::{json, Value};

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
                let crlf = content.contains("\r\n");
                let stripped = content.replace("\r\n", "");
                let line_ending = if stripped.contains('\r') || (crlf && stripped.contains('\n')) {
                    "mixed"
                } else if crlf {
                    "crlf"
                } else {
                    "lf"
                };
                json!({"kind": if size as u64 > FULL_FEATURE_MAX_BYTES { "limited" } else { "full" }, "content":content, "size":size, "lineEnding":line_ending})
            }
        },
    }
}
