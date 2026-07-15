use crate::file_content::{
    analyze_byte_content, ByteContent, FILE_ANALYSIS_BYTES, FULL_FEATURE_MAX_BYTES, HARD_CAP_BYTES,
};
use serde::Serialize;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

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
    Full {
        content: String,
        size: u64,
        line_ending: LineEnding,
    },
    #[serde(rename_all = "camelCase")]
    Limited {
        content: String,
        size: u64,
        line_ending: LineEnding,
    },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LineEnding {
    Lf,
    #[serde(rename = "crlf")]
    CrLf,
    Mixed,
}

fn detect_line_ending(content: &str) -> LineEnding {
    let bytes = content.as_bytes();
    let mut has_lf = false;
    let mut has_crlf = false;
    let mut has_bare_cr = false;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                has_crlf = true;
                index += 2;
            }
            b'\r' => {
                has_bare_cr = true;
                index += 1;
            }
            b'\n' => {
                has_lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }

    if has_bare_cr || (has_lf && has_crlf) {
        LineEnding::Mixed
    } else if has_crlf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
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
                    let line_ending = detect_line_ending(&content);
                    if size > FULL_FEATURE_MAX_BYTES {
                        Ok(OpenFileResult::Limited {
                            content,
                            size,
                            line_ending,
                        })
                    } else {
                        Ok(OpenFileResult::Full {
                            content,
                            size,
                            line_ending,
                        })
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

// 純字面（不碰檔案系統）正規化：吃掉 "." 、對 ".." 做 pop。因為新建/改名的目標
// 可能尚未存在（無法 canonicalize），字面解析先擋掉 ".." 逃逸。此函式本身不解析
// symlink——symlink component 逃逸另由 resolve_in_workspace canonicalize「已存在
// 的最深祖先」把關（見下方）。
fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// 從 target 逐層往上，回傳最深的、實際存在的祖先（含 target 本身）。用
// symlink_metadata（lstat）偵測存在，dangling symlink 也算存在——這樣穿越
// 外部 symlink 的路徑會停在該 symlink component 上，交給呼叫端 canonicalize
// 檢查。root 必然存在且是 target 的祖先，迴圈至少停在 root。
fn deepest_existing_ancestor(target: &Path) -> PathBuf {
    let mut cur = target;
    loop {
        if cur.symlink_metadata().is_ok() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return cur.to_path_buf(),
        }
    }
}

// 把前端傳來的目標 path 綁進 workspace 邊界：
//   1. canonicalize workspace root（實際存在、解析 symlink）。
//   2. 字面正規化目標，要求它在 root 底下、且不是 root 本身（不允許對 workspace
//      根目錄本身建立/改名/刪除）——擋掉 path 參數帶 "../.." 的字面逃逸。
//   3. symlink 逃逸：字面 containment 檢查不到「path 中某個 component 是指向
//      workspace 外的 symlink」，實際 fs 操作會沿著它逃出邊界。canonicalize
//      目標「已存在的最深祖先」（要新建的尾段本身尚不存在，無法 canonicalize），
//      確認其真實位置仍在 root 底下；剩餘尚不存在的尾段已由 lexical 正規化保證
//      不含 ".."，只會在 root 內往下建立。
// 回傳正規化後的絕對路徑供後續 fs 操作使用。
fn resolve_in_workspace(workspace: &str, path: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(workspace).map_err(|e| format!("invalid workspace: {e}"))?;
    let target = normalize_lexical(Path::new(path));
    if target == root {
        return Err("refusing to operate on the workspace root".into());
    }
    if !target.starts_with(&root) {
        return Err("path escapes the workspace".into());
    }
    let existing = deepest_existing_ancestor(&target);
    let canonical = std::fs::canonicalize(&existing).map_err(|e| format!("invalid path: {e}"))?;
    if !canonical.starts_with(&root) {
        return Err("path escapes the workspace via symlink".into());
    }
    Ok(target)
}

#[tauri::command]
pub fn fs_create_file(workspace: String, path: String) -> Result<(), String> {
    let target = resolve_in_workspace(&workspace, &path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create parent dir failed: {e}"))?;
    }
    // create_new 讓「檢查不存在＋建立」成為單一原子操作，避免 TOCTOU 覆蓋。
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("file already exists: {}", target.display())
            } else {
                format!("create file failed: {e}")
            }
        })?;
    Ok(())
}

#[tauri::command]
pub fn fs_create_dir(workspace: String, path: String) -> Result<(), String> {
    let target = resolve_in_workspace(&workspace, &path)?;
    if target.exists() {
        return Err(format!("directory already exists: {}", target.display()));
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("create dir failed: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn fs_rename(workspace: String, from: String, to: String) -> Result<(), String> {
    let src = resolve_in_workspace(&workspace, &from)?;
    let dst = resolve_in_workspace(&workspace, &to)?;
    if !src.exists() {
        return Err(format!("source does not exist: {}", src.display()));
    }
    if dst.exists() {
        return Err(format!("target already exists: {}", dst.display()));
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("rename failed: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn fs_delete(workspace: String, path: String) -> Result<(), String> {
    let target = resolve_in_workspace(&workspace, &path)?;
    // symlink_metadata：symlink 一律當檔案處理（remove_file 只砍連結、不遞迴刪
    // 連結目標）。
    let meta = std::fs::symlink_metadata(&target).map_err(|e| format!("stat failed: {e}"))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("remove dir failed: {e}"))?;
    } else {
        std::fs::remove_file(&target).map_err(|e| format!("remove file failed: {e}"))?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct FileBase64 {
    pub data: String,
    pub size: u64,
}

/// Reads a user-picked file (AgentZone image attachments) as base64. The
/// caller enforces the mime whitelist by extension; this side enforces only
/// the size ceiling so an oversized pick fails with a structured error
/// instead of ballooning the IPC payload.
#[tauri::command]
pub fn read_file_base64(path: String, max_bytes: u64) -> Result<FileBase64, String> {
    use base64::Engine;
    let meta = std::fs::metadata(&path).map_err(|e| format!("stat failed: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a regular file: {path}"));
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "file too large: {} bytes (max {max_bytes})",
            meta.len()
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("read failed: {e}"))?;
    Ok(FileBase64 {
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        size: bytes.len() as u64,
    })
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
    fn detects_editable_line_endings_without_changing_content() {
        for (name, bytes, expected) in [
            ("lf.txt", b"one\ntwo\n".as_slice(), LineEnding::Lf),
            ("crlf.txt", b"one\r\ntwo\r\n".as_slice(), LineEnding::CrLf),
            ("mixed.txt", b"one\r\ntwo\n".as_slice(), LineEnding::Mixed),
            ("bare-cr.txt", b"one\rtwo".as_slice(), LineEnding::Mixed),
            ("empty.txt", b"".as_slice(), LineEnding::Lf),
            ("no-newline.txt", b"one".as_slice(), LineEnding::Lf),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join(name);
            fs::write(&path, bytes).unwrap();
            match classify_and_read(&path).unwrap() {
                OpenFileResult::Full {
                    content,
                    line_ending,
                    ..
                } => {
                    assert_eq!(content.as_bytes(), bytes);
                    assert_eq!(line_ending, expected);
                }
                other => panic!("expected Full, got {other:?}"),
            }
        }
    }

    #[test]
    fn editable_line_ending_contract_serializes_as_typescript_shape() {
        let value = serde_json::to_value(OpenFileResult::Full {
            content: "one\r\ntwo\r\n".into(),
            size: 10,
            line_ending: LineEnding::CrLf,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "full",
                "content": "one\r\ntwo\r\n",
                "size": 10,
                "lineEnding": "crlf"
            })
        );
    }

    #[test]
    fn utf8_bom_content_and_line_ending_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("utf8-bom.txt");
        let bytes = b"\xef\xbb\xbfone\r\ntwo\r\n";
        fs::write(&path, bytes).unwrap();
        match classify_and_read(&path).unwrap() {
            OpenFileResult::Full {
                content,
                line_ending,
                ..
            } => {
                assert_eq!(content.as_bytes(), bytes);
                assert_eq!(line_ending, LineEnding::CrLf);
            }
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn open_file_limited_between_thresholds() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let mid = tmp.path().join("mid.txt");
        // 實寫文字內容（稀疏檔的 NUL 會被 sniff 判成 Binary，不可用 set_len）
        let block = "abcdefghij\r\n".repeat(86);
        let mut f = std::io::BufWriter::new(fs::File::create(&mid).unwrap());
        let times = crate::file_content::FULL_FEATURE_MAX_BYTES / 1024 + 2;
        for _ in 0..times {
            f.write_all(block.as_bytes()).unwrap();
        }
        drop(f);
        match classify_and_read(&mid).unwrap() {
            OpenFileResult::Limited { line_ending, .. } => {
                assert_eq!(line_ending, LineEnding::CrLf)
            }
            other => panic!("expected Limited, got {other:?}"),
        }
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

    // 生產環境 workspace 一律先過 open_workspace → canonicalize（macOS 上 tempdir
    // 的 /var 是 /private/var 的 symlink），測試也照此把 root canonicalize 後當
    // workspace 傳，才與實際邊界檢查一致。
    fn ws(tmp: &tempfile::TempDir) -> String {
        fs::canonicalize(tmp.path())
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn under(workspace: &str, rel: &str) -> String {
        format!("{workspace}/{rel}")
    }

    #[test]
    fn fs_create_file_creates_with_parent_dirs_and_rejects_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let target = under(&w, "nested/deep/a.txt");
        fs_create_file(w.clone(), target.clone()).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "");
        // 已存在 → 錯誤，且內容不被覆蓋。
        fs::write(&target, "keep").unwrap();
        assert!(fs_create_file(w.clone(), target.clone()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn fs_create_dir_creates_and_rejects_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let target = under(&w, "newdir");
        fs_create_dir(w.clone(), target.clone()).unwrap();
        assert!(Path::new(&target).is_dir());
        assert!(fs_create_dir(w.clone(), target.clone()).is_err());
    }

    #[test]
    fn fs_rename_moves_and_rejects_existing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let from = under(&w, "old.txt");
        let to = under(&w, "new.txt");
        fs::write(&from, "body").unwrap();
        fs_rename(w.clone(), from.clone(), to.clone()).unwrap();
        assert!(!Path::new(&from).exists());
        assert_eq!(fs::read_to_string(&to).unwrap(), "body");
        // to 已存在 → 錯誤。
        let from2 = under(&w, "other.txt");
        fs::write(&from2, "x").unwrap();
        assert!(fs_rename(w.clone(), from2.clone(), to.clone()).is_err());
        assert!(Path::new(&from2).exists());
    }

    #[test]
    fn fs_delete_removes_file_and_dir_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let file = under(&w, "f.txt");
        fs::write(&file, "x").unwrap();
        fs_delete(w.clone(), file.clone()).unwrap();
        assert!(!Path::new(&file).exists());

        let dir = under(&w, "d");
        fs::create_dir_all(format!("{dir}/sub")).unwrap();
        fs::write(format!("{dir}/sub/inner.txt"), "y").unwrap();
        fs_delete(w.clone(), dir.clone()).unwrap();
        assert!(!Path::new(&dir).exists());
    }

    #[test]
    fn fs_commands_reject_paths_escaping_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        let escape = under(&w, "../escaped.txt");
        assert!(fs_create_file(w.clone(), escape.clone()).is_err());
        assert!(fs_create_dir(w.clone(), escape.clone()).is_err());
        assert!(fs_delete(w.clone(), escape.clone()).is_err());
        assert!(fs_rename(w.clone(), under(&w, "a.txt"), escape.clone()).is_err());
        // 目標檔案未被建立在 workspace 之外。
        assert!(!Path::new(&fs::canonicalize(&tmp).unwrap())
            .parent()
            .unwrap()
            .join("escaped.txt")
            .exists());
    }

    #[test]
    fn fs_commands_reject_workspace_root_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let w = ws(&tmp);
        assert!(fs_delete(w.clone(), w.clone()).is_err());
        assert!(Path::new(&w).is_dir());
    }

    // workspace 內放一個指向外部目錄的 symlink，斷言透過它（穿越 symlink component）
    // 的 create/rename/delete 全部 fail-closed，且外部目錄不被寫入/破壞。
    #[cfg(unix)]
    #[test]
    fn fs_commands_reject_paths_through_external_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_root = fs::canonicalize(outside.path()).unwrap();
        let w = ws(&tmp);

        // <workspace>/link -> <外部目錄>
        let link = under(&w, "link");
        symlink(&outside_root, &link).unwrap();

        // 透過 symlink 建立檔案 / 目錄：擋，且外部不出現該項目。
        let through_file = under(&w, "link/evil.txt");
        assert!(fs_create_file(w.clone(), through_file.clone()).is_err());
        assert!(fs_create_dir(w.clone(), under(&w, "link/evildir")).is_err());
        assert!(!outside_root.join("evil.txt").exists());
        assert!(!outside_root.join("evildir").exists());

        // 外部先放一個真實檔案，確認 delete / rename-from 穿越 symlink 都擋，
        // 且不是因為來源不存在才失敗——檔案仍在外部原地。
        let outside_file = outside_root.join("real.txt");
        fs::write(&outside_file, "keep").unwrap();
        assert!(fs_delete(w.clone(), under(&w, "link/real.txt")).is_err());
        assert!(fs_rename(
            w.clone(),
            under(&w, "link/real.txt"),
            under(&w, "moved.txt")
        )
        .is_err());
        assert!(outside_file.exists());
        assert_eq!(fs::read_to_string(&outside_file).unwrap(), "keep");

        // rename-to 穿越 symlink 也擋，來源留在 workspace 內。
        let src = under(&w, "src.txt");
        fs::write(&src, "body").unwrap();
        assert!(fs_rename(w.clone(), src.clone(), through_file.clone()).is_err());
        assert!(Path::new(&src).exists());
        assert!(!outside_root.join("evil.txt").exists());
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
