/// 讀檔前只 sniff 前 1KB（做法借自 Zed 的 FILE_ANALYSIS_BYTES）
pub const FILE_ANALYSIS_BYTES: usize = 1024;
/// <= 此值：full（LSP、syntax 全開）。Spike B 校準。
pub const FULL_FEATURE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// > 此值：too large，不載入。Spike B 校準。
pub const HARD_CAP_BYTES: u64 = 50 * 1024 * 1024;
/// 單行超過此字元數：該檔 syntax highlight 停用。Spike B 校準。
pub const MAX_LINE_LEN_SYNTAX_OFF: usize = 10_000;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ByteContent {
    Text,
    Utf16Le,
    Utf16Be,
    Binary,
}

const MAGIC: &[&[u8]] = &[
    b"%PDF-",
    b"\x89PNG\r\n\x1a\n",
    b"PK\x03\x04",
    b"PK\x05\x06",
    b"PK\x07\x08",
    b"\xff\xd8\xff",
    b"GIF87a",
    b"GIF89a",
    b"OggS",
    b"fLaC",
    b"RIFF",
    b"\x7fELF",
];

fn is_known_binary_header(bytes: &[u8]) -> bool {
    MAGIC.iter().any(|m| bytes.starts_with(m))
}

pub fn analyze_byte_content(prefix: &[u8]) -> ByteContent {
    if prefix.is_empty() {
        return ByteContent::Text;
    }
    if prefix.starts_with(b"\xff\xfe") {
        return ByteContent::Utf16Le;
    }
    if prefix.starts_with(b"\xfe\xff") {
        return ByteContent::Utf16Be;
    }
    if is_known_binary_header(prefix) {
        return ByteContent::Binary;
    }
    if prefix.contains(&0u8) {
        // 無 BOM 且含 NUL：MVP 保守判 Binary（無 BOM 的 UTF-16 heuristic 列 v2）
        return ByteContent::Binary;
    }
    // 非文字樣態位元組（不可印 ASCII 控制字元，排除 tab/LF/CR/FF）比例 > 8% → Binary
    let suspicious = prefix
        .iter()
        .filter(|&&b| b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r' && b != 0x0c)
        .count();
    if suspicious * 100 > prefix.len() * 8 {
        return ByteContent::Binary;
    }
    ByteContent::Text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_text_is_text() {
        assert_eq!(
            analyze_byte_content("fn main() {}\n".as_bytes()),
            ByteContent::Text
        );
        assert_eq!(
            analyze_byte_content("中文內容 with mixed ascii\n".as_bytes()),
            ByteContent::Text
        );
    }

    #[test]
    fn known_magic_bytes_are_binary() {
        assert_eq!(analyze_byte_content(b"%PDF-1.7 rest"), ByteContent::Binary);
        assert_eq!(
            analyze_byte_content(b"\x89PNG\r\n\x1a\n"),
            ByteContent::Binary
        );
        assert_eq!(
            analyze_byte_content(b"PK\x03\x04zipdata"),
            ByteContent::Binary
        );
        assert_eq!(
            analyze_byte_content(b"\xff\xd8\xffjpegdata"),
            ByteContent::Binary
        );
        assert_eq!(analyze_byte_content(b"GIF89a...."), ByteContent::Binary);
    }

    #[test]
    fn utf16_bom_detected() {
        assert_eq!(
            analyze_byte_content(b"\xff\xfeh\x00i\x00"),
            ByteContent::Utf16Le
        );
        assert_eq!(
            analyze_byte_content(b"\xfe\xff\x00h\x00i"),
            ByteContent::Utf16Be
        );
    }

    #[test]
    fn nul_bytes_without_bom_are_binary() {
        let mut v = b"ELF".to_vec();
        v.extend(std::iter::repeat_n(0u8, 64));
        v.extend(b"data section");
        assert_eq!(analyze_byte_content(&v), ByteContent::Binary);
    }

    #[test]
    fn mostly_non_text_bytes_are_binary() {
        let v: Vec<u8> = (1u8..=255).cycle().take(512).collect();
        assert_eq!(analyze_byte_content(&v), ByteContent::Binary);
    }

    #[test]
    fn empty_is_text() {
        assert_eq!(analyze_byte_content(b""), ByteContent::Text);
    }
}
