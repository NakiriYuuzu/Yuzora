// M2 Task 3: porcelain v2 parser

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileEntry {
    pub path: String,
    pub orig_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedStatus {
    pub branch: Option<String>,
    pub head_oid: String,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<GitFileEntry>,
    pub unstaged: Vec<GitFileEntry>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<GitFileEntry>,
}

pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<ParsedStatus, String> {
    let mut s = ParsedStatus {
        head_oid: String::new(),
        ..Default::default()
    };
    let mut records = bytes
        .split(|b| *b == 0)
        .map(|r| String::from_utf8_lossy(r).into_owned());
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(rest) = rec.strip_prefix("# ") {
            let mut it = rest.splitn(2, ' ');
            match (it.next().unwrap_or(""), it.next().unwrap_or("")) {
                ("branch.oid", v) => s.head_oid = v.to_string(),
                ("branch.head", "(detached)") => s.detached = true,
                ("branch.head", v) => s.branch = Some(v.to_string()),
                ("branch.upstream", v) => s.upstream = Some(v.to_string()),
                ("branch.ab", v) => {
                    for part in v.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            s.ahead = n.parse().unwrap_or(0)
                        }
                        if let Some(n) = part.strip_prefix('-') {
                            s.behind = n.parse().unwrap_or(0)
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.chars().next().unwrap_or(' ');
        match kind {
            '1' | '2' => {
                // 1: 8 個空白分欄後為 path；2: 第 9 欄為 X<score>，其後 path，orig 在下一個 NUL 記錄
                let field_count = if kind == '1' { 8 } else { 9 };
                let mut parts = rec.splitn(field_count + 1, ' ');
                let mut fields: Vec<&str> = Vec::with_capacity(field_count + 1);
                for _ in 0..=field_count {
                    fields.push(
                        parts
                            .next()
                            .ok_or_else(|| format!("malformed record: {rec}"))?,
                    )
                }
                let xy = fields[1];
                let path = fields[field_count].to_string();
                let orig = if kind == '2' {
                    Some(records.next().ok_or("rename record missing orig path")?)
                } else {
                    None
                };
                let x = xy.chars().next().unwrap_or('.');
                let y = xy.chars().nth(1).unwrap_or('.');
                if x != '.' {
                    s.staged.push(GitFileEntry {
                        path: path.clone(),
                        orig_path: orig.clone(),
                        status: x.to_string(),
                    })
                }
                if y != '.' {
                    s.unstaged.push(GitFileEntry {
                        path,
                        orig_path: orig,
                        status: y.to_string(),
                    })
                }
            }
            'u' => {
                let path = rec
                    .splitn(11, ' ')
                    .nth(10)
                    .ok_or_else(|| format!("malformed u record: {rec}"))?;
                s.conflicted.push(GitFileEntry {
                    path: path.to_string(),
                    orig_path: None,
                    status: "U".into(),
                })
            }
            '?' => {
                let path = rec
                    .strip_prefix("? ")
                    .ok_or_else(|| format!("malformed untracked record: {rec}"))?;
                if path.is_empty() {
                    return Err(format!("malformed untracked record: {rec}"));
                }
                s.untracked.push(path.to_string())
            }
            '!' => {}
            _ => return Err(format!("unknown record type: {rec}")),
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn z(records: &[&str]) -> Vec<u8> {
        records.join("\0").into_bytes()
    }

    #[test]
    fn parses_branch_header_and_ab() {
        let input = z(&[
            "# branch.oid 1234abcd",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
            "",
        ]);
        let s = parse_porcelain_v2(&input).unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 3));
        assert!(!s.detached);
    }

    #[test]
    fn detached_head_sets_flag_and_no_branch() {
        let input = z(&["# branch.oid 1234abcd", "# branch.head (detached)", ""]);
        let s = parse_porcelain_v2(&input).unwrap();
        assert!(s.detached);
        assert_eq!(s.branch, None);
    }

    #[test]
    fn ordinary_entry_splits_staged_and_unstaged() {
        let input = z(&[
            "# branch.oid x",
            "# branch.head main",
            "1 MM N... 100644 100644 100644 aaaa bbbb src/a.rs",
            "",
        ]);
        let s = parse_porcelain_v2(&input).unwrap();
        assert_eq!(
            s.staged,
            vec![GitFileEntry {
                path: "src/a.rs".into(),
                orig_path: None,
                status: "M".into()
            }]
        );
        assert_eq!(s.unstaged[0].path, "src/a.rs");
    }

    #[test]
    fn rename_entry_carries_orig_path_with_space_names() {
        let input = z(&[
            "# branch.oid x",
            "# branch.head main",
            "2 R. N... 100644 100644 100644 aaaa bbbb R100 new name.txt",
            "old name.txt",
            "",
        ]);
        let s = parse_porcelain_v2(&input).unwrap();
        assert_eq!(s.staged[0].path, "new name.txt");
        assert_eq!(s.staged[0].orig_path.as_deref(), Some("old name.txt"));
        assert_eq!(s.staged[0].status, "R");
        assert!(s.unstaged.is_empty());
    }

    #[test]
    fn unmerged_and_untracked_and_ignored() {
        let input = z(&[
            "# branch.oid x",
            "# branch.head main",
            "u UU N... 100644 100644 100644 100644 a1 a2 a3 conflict.rs",
            "? new file.txt",
            "! target",
            "",
        ]);
        let s = parse_porcelain_v2(&input).unwrap();
        assert_eq!(s.conflicted[0].path, "conflict.rs");
        assert_eq!(s.untracked, vec!["new file.txt".to_string()]);
        assert!(s.staged.is_empty());
    }

    #[test]
    fn malformed_record_is_error_not_panic() {
        assert!(parse_porcelain_v2(&z(&["1 MM", ""])).is_err());
    }

    #[test]
    fn bare_untracked_record_is_error_not_panic() {
        let input = z(&["# branch.head main", "?", ""]);
        assert!(parse_porcelain_v2(&input).is_err());
    }

    #[test]
    fn untracked_record_with_empty_path_is_error() {
        let input = z(&["# branch.head main", "? ", ""]);
        assert!(parse_porcelain_v2(&input).is_err());
    }
}
