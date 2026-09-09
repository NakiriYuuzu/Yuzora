//! One dedicated stdio channel per terminal or event subscription.
use crate::herdr_service::{
    HerdrScrollDirection, HerdrSubscriptionEvent, HerdrTerminalEvent, HerdrTerminalMode,
};
use crate::protocol::{ConnectionOwner, Outcome, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};

pub const STREAM_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum StreamConfig {
    Git {
        path: String,
        repository_root: String,
    },
    Search {
        path: String,
        query: String,
        case_sensitive: bool,
    },
    Files {
        path: String,
    },
    Terminal {
        binary: String,
        session_name: String,
        target: String,
        mode: HerdrTerminalMode,
        takeover: bool,
        cols: u16,
        rows: u16,
    },
    Events {
        binary: String,
        session_name: String,
        #[serde(default)]
        pane_ids: Vec<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum StreamCommand {
    Open {
        config: StreamConfig,
    },
    Input {
        text: Option<String>,
        bytes_base64: Option<String>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Scroll {
        direction: HerdrScrollDirection,
        lines: u32,
    },
    Close,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamRequest {
    pub version: u32,
    pub owner: ConnectionOwner,
    pub id: String,
    pub operation: StreamCommand,
}

impl StreamRequest {
    pub fn validate(&self, owner: &ConnectionOwner) -> Result<(), String> {
        if self.version != PROTOCOL_VERSION {
            return Err("protocol-mismatch".into());
        }
        if &self.owner != owner {
            return Err("connection-owner-mismatch".into());
        }
        if self.id.is_empty() || self.id.len() > 256 {
            return Err("invalid-request-id".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamPayload {
    #[serde(rename_all = "camelCase")]
    Git {
        workspace_root: String,
    },
    Search {
        event: crate::search::SearchEvent,
    },
    #[serde(rename_all = "camelCase")]
    Files {
        workspace_root: String,
        paths: Vec<String>,
    },
    Reply {
        id: String,
        outcome: Outcome,
    },
    Terminal {
        event: HerdrTerminalEvent,
    },
    Subscription {
        event: HerdrSubscriptionEvent,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamFrame {
    pub version: u32,
    pub owner: ConnectionOwner,
    pub payload: StreamPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_commands_cannot_cross_generation_or_protocol() {
        let owner = ConnectionOwner {
            host_id: "a".into(),
            generation: 2,
        };
        let mut request = StreamRequest {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            id: "1".into(),
            operation: StreamCommand::Close,
        };
        assert!(request.validate(&owner).is_ok());
        request.owner.generation = 1;
        assert_eq!(
            request.validate(&owner).unwrap_err(),
            "connection-owner-mismatch"
        );
        request.owner = owner.clone();
        request.version += 1;
        assert_eq!(request.validate(&owner).unwrap_err(), "protocol-mismatch");
    }
}
