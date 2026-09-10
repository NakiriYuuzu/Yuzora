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
        "filesRead",
        "filesWrite",
        "filesCreate",
        "filesRename",
        "filesDelete",
        "filesReadBase64",
        "herdrDiscover",
        "herdrRequest",
        "herdrCall",
        "herdrMetadata",
        "herdrStart",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
