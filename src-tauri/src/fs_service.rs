use crate::file_content::{
    analyze_byte_content, ByteContent, FILE_ANALYSIS_BYTES, FULL_FEATURE_MAX_BYTES, HARD_CAP_BYTES,
};
use serde::Serialize;
use std::io::Read;
use std::path::Path;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

pub fn list_dir_entries(dir: &Path) -> Result<Vec<FileNode>, String> {
    let mut nodes: Vec<FileNode> = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir failed: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().ok()?.is_dir();
            Some(FileNode {
                name,
                path: path.to_string_lossy().into_owned(),
                is_dir,
            })
        })
        .collect();
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(nodes)
}

pub fn canonicalize_workspace(path: &str) -> Result<String, String> {
    let p = std::fs::canonicalize(path).map_err(|e| format!("invalid path: {e}"))?;
    if !p.is_dir() {
        return Err("workspace path is not a directory".into());
    }
    Ok(p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn open_workspace(path: String) -> Result<String, String> {
    canonicalize_workspace(&path)
}

#[tauri::command]
pub fn list_dir(path: String) -> Result<Vec<FileNode>, String> {
    list_dir_entries(Path::new(&path))
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpenFileResult {
    #[serde(rename_all = "camelCase")]
    Full { content: String, size: u64 },
    #[serde(rename_all = "camelCase")]
    Limited { content: String, size: u64 },
    #[serde(rename_all = "camelCase")]
    TooLarge { size: u64 },
    #[serde(rename_all = "camelCase")]
    Binary { size: u64 },
    #[serde(rename_all = "camelCase")]
    NonUtf8Readonly {
        content: String,
        encoding: String,
        size: u64,
    },
}

pub fn classify_and_read(path: &Path) -> Result<OpenFileResult, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    let size = meta.len();
    if size > HARD_CAP_BYTES {
        return Ok(OpenFileResult::TooLarge { size });
    }

    let mut file = std::fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut prefix = vec![0u8; FILE_ANALYSIS_BYTES.min(size as usize)];
    file.read_exact(&mut prefix)
        .map_err(|e| format!("read failed: {e}"))?;

    match analyze_byte_content(&prefix) {
        ByteContent::Binary => Ok(OpenFileResult::Binary { size }),
        ByteContent::Utf16Le | ByteContent::Utf16Be => {
            let bytes = read_rest(&mut file, prefix, size)?;
            let codec =
                if analyze_byte_content(&bytes[..bytes.len().min(2)]) == ByteContent::Utf16Be {
                    encoding_rs::UTF_16BE
                } else {
                    encoding_rs::UTF_16LE
                };
            let (cow, used, _) = codec.decode(&bytes);
            Ok(OpenFileResult::NonUtf8Readonly {
                content: cow.into_owned(),
                encoding: used.name().to_string(),
                size,
            })
        }
        ByteContent::Text => {
            let bytes = read_rest(&mut file, prefix, size)?;
            match String::from_utf8(bytes) {
                Ok(content) => {
                    if size > FULL_FEATURE_MAX_BYTES {
                        Ok(OpenFileResult::Limited { content, size })
                    } else {
                        Ok(OpenFileResult::Full { content, size })
                    }
                }
                Err(err) => {
                    // 非 UTF-8 且無 BOM：以 WINDOWS_1252 lossy 解碼供唯讀檢視
                    // 用 decode_without_bom_handling：避免內容中恰好出現 UTF-8/UTF-16 BOM 位元組時
                    // 被 decode() 嗅探並覆蓋成該編碼解碼（decode() 的 BOM 嗅探是為一般用途設計，
                    // 這裡需要的是固定逐位元組 WINDOWS_1252 解碼）
                    let bytes = err.into_bytes();
                    let (cow, _had_errors) =
                        encoding_rs::WINDOWS_1252.decode_without_bom_handling(&bytes);
                    Ok(OpenFileResult::NonUtf8Readonly {
                        content: cow.into_owned(),
                        encoding: encoding_rs::WINDOWS_1252.name().to_string(),
                        size,
                    })
                }
            }
        }
    }
}

fn read_rest(file: &mut std::fs::File, mut prefix: Vec<u8>, size: u64) -> Result<Vec<u8>, String> {
    prefix.reserve(size as usize - prefix.len());
    file.read_to_end(&mut prefix)
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(prefix)
}

pub fn write_file(path: &str, content: &str) -> Result<u64, String> {
    std::fs::write(path, content).map_err(|e| format!("write failed: {e}"))?;
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    let mtime = meta
        .modified()
        .map_err(|e| format!("mtime failed: {e}"))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("time error: {e}"))?
        .as_millis() as u64;
    Ok(mtime)
}

#[tauri::command]
pub fn open_file(path: String) -> Result<OpenFileResult, String> {
    classify_and_read(Path::new(&path))
}

#[tauri::command]
pub fn save_file(path: String, content: String) -> Result<u64, String> {
    write_file(&path, &content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_dir_sorts_dirs_first_then_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("zeta")).unwrap();
        fs::write(tmp.path().join("alpha.txt"), "a").unwrap();
        fs::write(tmp.path().join("Beta.txt"), "b").unwrap();
        let nodes = list_dir_entries(tmp.path()).unwrap();
        let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha.txt", "Beta.txt"]);
        assert!(nodes[0].is_dir);
    }

    #[test]
    fn canonicalize_workspace_rejects_files() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        fs::write(&f, "x").unwrap();
        assert!(canonicalize_workspace(f.to_str().unwrap()).is_err());
        assert!(canonicalize_workspace(tmp.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn open_file_classifies_by_size_and_content() {
        let tmp = tempfile::tempdir().unwrap();

        let small = tmp.path().join("small.ts");
        fs::write(&small, "export const a = 1\n").unwrap();
        match classify_and_read(&small).unwrap() {
            OpenFileResult::Full { content, .. } => assert!(content.contains("a = 1")),
            other => panic!("expected Full, got {other:?}"),
        }

        let png = tmp.path().join("img.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert!(matches!(
            classify_and_read(&png).unwrap(),
            OpenFileResult::Binary { .. }
        ));

        let big = tmp.path().join("big.txt");
        let f = fs::File::create(&big).unwrap();
        f.set_len(crate::file_content::HARD_CAP_BYTES + 1).unwrap();
        assert!(matches!(
            classify_and_read(&big).unwrap(),
            OpenFileResult::TooLarge { .. }
        ));
    }

    #[test]
    fn open_file_limited_between_thresholds() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let mid = tmp.path().join("mid.txt");
        // 實寫文字內容（稀疏檔的 NUL 會被 sniff 判成 Binary，不可用 set_len）
        let block = "abcdefghij".repeat(103);
        let mut f = std::io::BufWriter::new(fs::File::create(&mid).unwrap());
        let times = crate::file_content::FULL_FEATURE_MAX_BYTES / 1024 + 2;
        for _ in 0..times {
            f.write_all(block.as_bytes()).unwrap();
        }
        drop(f);
        assert!(matches!(
            classify_and_read(&mid).unwrap(),
            OpenFileResult::Limited { .. }
        ));
    }

    #[test]
    fn save_file_writes_and_returns_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.txt");
        let mtime = write_file(p.to_str().unwrap(), "hello").unwrap();
        assert!(mtime > 0);
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn open_file_utf16_bom_is_readonly() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("utf16.txt");
        fs::write(&p, b"\xff\xfeh\x00i\x00").unwrap();
        match classify_and_read(&p).unwrap() {
            OpenFileResult::NonUtf8Readonly {
                content, encoding, ..
            } => {
                assert_eq!(content, "hi");
                assert!(encoding.contains("UTF-16"));
            }
            other => panic!("expected NonUtf8Readonly, got {other:?}"),
        }
    }

    #[test]
    fn open_file_non_utf8_falls_back_windows_1252() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("mixed.txt");
        // UTF-8 BOM (EF BB BF) + 非法 UTF-8 序列（C3 後接 0x28，不是合法 continuation byte）+ "AB"
        fs::write(&p, b"\xef\xbb\xbf\xc3\x28AB").unwrap();
        match classify_and_read(&p).unwrap() {
            OpenFileResult::NonUtf8Readonly {
                content, encoding, ..
            } => {
                assert_eq!(encoding, "windows-1252");
                // 逐位元組 Windows-1252（byte >= 0xA0 時與 Latin-1 相同，碼位＝位元組值）解碼：
                // 0xEF -> ï(U+00EF)  0xBB -> »(U+00BB)  0xBF -> ¿(U+00BF)  0xC3 -> Ã(U+00C3)
                // 0x28/0x41/0x42 為 ASCII，直接對應 "(AB"。
                // 修復後 UTF-8 BOM 不再被 decode() 特殊嗅探並覆蓋 codec，逐 byte 走 windows-1252，
                // 故 BOM 三 bytes 也被解成 "ï»¿" 而非被剝除——此為預期行為（唯讀 fallback 用途）。
                assert_eq!(content, "ï»¿Ã(AB");
            }
            other => panic!("expected NonUtf8Readonly, got {other:?}"),
        }
    }
}
