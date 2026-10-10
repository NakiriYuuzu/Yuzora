use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeKey {
    pub host_id: String,
    pub session_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionOwner {
    pub host_id: String,
    pub generation: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub id: String,
    pub owner: ConnectionOwner,
    pub operation: Operation,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Hello,
    ClipboardImage {
        png_base64: String,
    },
    Trust {
        call: crate::trust_command::TrustCommand,
    },
    WorkspaceAuthorize {
        workspace: String,
    },
    Git {
        workspace: String,
        repository_root: Option<String>,
        call: crate::git_command::GitCommand,
    },
    WorkspaceOpen {
        path: String,
    },
    WorkspaceClose {
        workspace: String,
    },
    FileNameSearch {
        workspace: String,
        query: String,
    },
    FilesList {
        workspace: String,
        path: String,
    },
    FilesRead {
        workspace: String,
        path: String,
    },
    FilesWrite {
        workspace: String,
        path: String,
        content: String,
        revision: String,
    },
    FilesCreate {
        workspace: String,
        path: String,
        directory: bool,
    },
    FilesRename {
        workspace: String,
        from: String,
        to: String,
    },
    FilesDelete {
        workspace: String,
        path: String,
    },
    FilesCopy {
        workspace: String,
        sources: Vec<String>,
        target_dir: String,
    },
    FilesMove {
        workspace: String,
        sources: Vec<String>,
        target_dir: String,
    },
    /// Copies absolute host paths into the workspace. Only the desktop app's
    /// WSL drop/paste commands send it, after authorising `sources` itself.
    FilesImport {
        workspace: String,
        sources: Vec<String>,
        target_dir: String,
    },
    FilesReadBase64 {
        workspace: String,
        path: String,
        max_bytes: u64,
    },
    HerdrDiscover {
        binary: String,
    },
    HerdrCall {
        binary: String,
        call: crate::herdr_command::HerdrCommand,
    },
    HerdrMetadata {
        binary: String,
        query: crate::herdr_backend::HerdrMetadata,
        session: Option<String>,
    },
    HerdrStart {
        binary: String,
    },
    HerdrRequest {
        socket: String,
        request: serde_json::Value,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub version: u32,
    pub id: String,
    pub owner: ConnectionOwner,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Outcome {
    Ok { value: serde_json::Value },
    Error { code: String, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub methods: Vec<String>,
}

pub fn methods() -> Vec<String> {
    [
        "hello",
        "clipboardImage",
        "tcpTunnel",
        "sqlite",
        "trust",
        "git",
        "workspaceAuthorize",
        "workspaceOpen",
        "workspaceClose",
        "filesList",
        "fileNameSearch",
        "filesRead",
        "filesWrite",
        "filesCreate",
        "filesRename",
        "filesDelete",
        "filesCopy",
        "filesMove",
        "filesImport",
        "filesReadBase64",
        "herdrDiscover",
        "herdrRequest",
        "herdrCall",
        "herdrMetadata",
        "herdrStart",
        // Not an operation: terminal streams accept a pointer cell on
        // `scroll`. A saved older helper rejects those fields and ends the
        // stream, so clients send them only when this is advertised.
        "herdrScrollCell",
        // Not an operation: terminal streams accept `mouse`. An older helper
        // cannot parse it and ends the stream, so clients gate on this.
        "herdrTerminalMouse",
        // Not an operation: `mouse` accepts action `move` (hover). An older
        // helper's action enum lacks it and the stream ends on parse failure.
        "herdrTerminalMouseMove",
        // `herdr_sessions_polled` is a distinct command an older helper lacks.
        "herdrSessionsPolled",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hello_advertises_capability_flags_older_helpers_lack() {
        for flag in ["herdrTerminalMouseMove", "herdrSessionsPolled"] {
            assert!(methods().iter().any(|name| name == flag), "{flag}");
        }
    }

    #[test]
    fn file_copy_and_move_use_the_documented_wire_shape() {
        // Fields stay snake_case like filesReadBase64's `max_bytes`.
        for (method, expected) in [("filesCopy", true), ("filesMove", false)] {
            let wire = json!({
                "method": method,
                "params": {"workspace": "w", "sources": ["a", "b/c"], "target_dir": "d"}
            });
            let operation: Operation = serde_json::from_value(wire.clone()).unwrap();
            match (&operation, expected) {
                (
                    Operation::FilesCopy {
                        workspace,
                        sources,
                        target_dir,
                    },
                    true,
                )
                | (
                    Operation::FilesMove {
                        workspace,
                        sources,
                        target_dir,
                    },
                    false,
                ) => {
                    assert_eq!(workspace, "w");
                    assert_eq!(sources, &["a", "b/c"]);
                    assert_eq!(target_dir, "d");
                }
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(serde_json::to_value(&operation).unwrap(), wire);
            assert!(methods().iter().any(|name| name == method));
        }
        assert!(serde_json::from_value::<Operation>(json!({
            "method": "filesCopy",
            "params": {"workspace": "w", "sources": [], "targetDir": "d"}
        }))
        .is_err());
    }

    #[test]
    fn files_import_is_advertised_and_uses_the_copy_wire_shape() {
        let wire = json!({
            "method": "filesImport",
            "params": {"workspace": "w", "sources": ["/mnt/c/a"], "target_dir": "d"}
        });
        let operation: Operation = serde_json::from_value(wire.clone()).unwrap();
        assert!(matches!(operation, Operation::FilesImport { .. }));
        assert_eq!(serde_json::to_value(&operation).unwrap(), wire);
        assert!(methods().iter().any(|name| name == "filesImport"));
    }
}
