use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize, Debug)]
pub struct LogEvent {
    pub level: String,
    pub kind: String,
    pub source: String,
    pub workspace_path: Option<String>,
    pub event: String,
    pub message: String,
    pub metadata: serde_json::Value,
}

pub fn mask_url_userinfo(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find("://") {
        let after_scheme = pos + 3;
        out.push_str(&rest[..after_scheme]);
        let tail = &rest[after_scheme..];
        // userinfo 只會出現在 authority 段（下一個 '/'、'?'、'#'、空白之前）
        let authority_end = tail
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("<redacted>");
            out.push_str(&authority[at..]);
        } else {
            out.push_str(authority);
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}
