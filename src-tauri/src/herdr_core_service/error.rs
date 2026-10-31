//! The error every `herdr_core_*` command rejects with: a stable `code` the frontend
//! switches on, a human-readable `message` for logs, and an optional `detail` (the core's own
//! error kind, a layout operation's code, ...). Texts shown to users are the frontend's.
use serde::Serialize;
use yuzora_host::herdr_core::client::ClientError;
use yuzora_host::herdr_core::docs::DocError;

use crate::runtime_preferences::SetSessionError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoreError {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CoreError {
    fn new(code: &'static str, message: impl Into<String>) -> CoreError {
        CoreError {
            code,
            message: message.into(),
            detail: None,
        }
    }

    /// The experimental runtime is switched off (or the app is exiting): nothing is
    /// connected and nothing is started.
    pub fn disabled() -> CoreError {
        CoreError::new("disabled", "the herdr-core runtime is switched off")
    }

    /// Switched on, but no compatible core is connected right now (see the status).
    pub fn not_connected() -> CoreError {
        CoreError::new("not_connected", "herdr-core is not connected")
    }

    /// The page that made the call was replaced (the main WebView started loading another)
    /// before it could be answered: nothing was kept for it.
    pub fn page_replaced() -> CoreError {
        CoreError::new(
            "page_replaced",
            "the page that asked was replaced before herdr-core answered",
        )
    }

    pub fn unknown_attach(id: &str) -> CoreError {
        CoreError::new("unknown_attach", format!("no herdr-core attach {id}"))
    }

    pub fn unknown_subscription(id: &str) -> CoreError {
        CoreError::new(
            "unknown_subscription",
            format!("no herdr-core event subscription {id}"),
        )
    }

    pub fn invalid_session(name: &str) -> CoreError {
        CoreError::new(
            "invalid_session",
            format!("herdr-core session {name:?} is not one of yuzora, default"),
        )
    }

    pub fn invalid_input(message: impl Into<String>) -> CoreError {
        CoreError::new("invalid_input", message)
    }

    pub fn preferences_unwritable(message: impl Into<String>) -> CoreError {
        CoreError::new("preferences_unwritable", message)
    }

    /// A debug build always uses its own session; `herdr_core_set_session` is not available.
    pub fn dev_session_fixed() -> CoreError {
        CoreError::new(
            "dev_session_fixed",
            "a debug build always uses the yuzora-dev herdr-core session",
        )
    }

    pub fn from_session_change(error: SetSessionError) -> CoreError {
        match error {
            SetSessionError::DevSessionFixed => CoreError::dev_session_fixed(),
            SetSessionError::Unwritable(message) => CoreError::preferences_unwritable(message),
        }
    }

    pub fn internal(message: impl Into<String>) -> CoreError {
        CoreError::new("internal", message)
    }

    /// `ClientError::code()` is the code; a core error also names its own `kind`.
    pub fn from_client(error: &ClientError) -> CoreError {
        let detail = match error {
            ClientError::Rpc(rpc) => Some(rpc.kind.clone()).filter(|kind| !kind.is_empty()),
            ClientError::Incompatible(refusal) => Some(refusal.code().to_string()),
            _ => None,
        };
        CoreError {
            code: error.code(),
            message: error.to_string(),
            detail,
        }
    }

    pub fn from_doc(error: &DocError) -> CoreError {
        let detail = match error {
            DocError::Op(op) => Some(op.code().to_string()),
            DocError::ReadOnly(reason) => Some(reason.code().to_string()),
            DocError::Rpc(rpc) => Some(rpc.kind.clone()).filter(|kind| !kind.is_empty()),
            DocError::Transport(code) => Some(code.clone()),
            _ => None,
        };
        CoreError {
            code: error.code(),
            message: error.to_string(),
            detail,
        }
    }

    /// The core said the terminal is not there: closing it twice is not an error.
    pub fn is_not_found(&self) -> bool {
        self.code == "rpc" && self.detail.as_deref() == Some("not_found")
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoreError {}

#[cfg(test)]
mod tests {
    use super::*;
    use yuzora_host::herdr_core::caps::Refusal;
    use yuzora_host::herdr_core::docs::OpError;
    use yuzora_host::herdr_core::rpc::RpcError;

    fn rpc(kind: &str) -> RpcError {
        RpcError {
            code: -32000,
            message: kind.into(),
            kind: kind.into(),
            hint: String::new(),
        }
    }

    #[test]
    fn herdr_core_errors_keep_the_hosts_stable_codes_and_the_cores_kind() {
        let from_rpc = CoreError::from_client(&ClientError::Rpc(rpc("not_found")));
        assert_eq!(
            (from_rpc.code, from_rpc.detail.as_deref()),
            ("rpc", Some("not_found"))
        );
        assert!(from_rpc.is_not_found());
        assert!(!CoreError::from_client(&ClientError::Rpc(rpc("live"))).is_not_found());
        assert_eq!(
            CoreError::from_client(&ClientError::ConnectionLost).code,
            "connection_lost"
        );
        assert_eq!(CoreError::from_client(&ClientError::Timeout).detail, None);
        let incompatible = CoreError::from_client(&ClientError::Incompatible(Refusal::TooOld {
            version: "0.1.0".into(),
        }));
        assert_eq!(
            (incompatible.code, incompatible.detail.as_deref()),
            ("incompatible", Some("too_old"))
        );

        let op = CoreError::from_doc(&DocError::Op(OpError::BadRatio));
        assert_eq!((op.code, op.detail.as_deref()), ("op", Some("bad_ratio")));
        let gave_up = CoreError::from_doc(&DocError::GaveUp { attempts: 5 });
        assert_eq!((gave_up.code, gave_up.detail), ("gave_up", None));
    }

    #[test]
    fn herdr_core_errors_serialize_as_code_message_and_optional_detail() {
        let plain = serde_json::to_value(CoreError::disabled()).unwrap();
        assert_eq!(plain["code"], "disabled");
        assert!(plain["message"].is_string());
        assert!(plain.get("detail").is_none());
        let with_detail =
            serde_json::to_value(CoreError::from_client(&ClientError::Rpc(rpc("live")))).unwrap();
        assert_eq!(with_detail["detail"], "live");
    }
}
