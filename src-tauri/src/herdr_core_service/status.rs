//! What the UI is told about the herdr-core connection: one status object, pushed on the
//! `herdr-core:status` event whenever it changes and returned by `herdr_core_status`.
use serde::Serialize;
use yuzora_host::herdr_core::caps::Refusal;
use yuzora_host::herdr_core::client::ClientError;
use yuzora_host::herdr_core::startup::{Origin, Ready, StartError};

/// The Tauri event carrying a [`CoreStatus`].
pub const STATUS_EVENT: &str = "herdr-core:status";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreState {
    /// The experimental switch is off: nothing is connected or started.
    Disabled,
    /// Looking for a core, starting the bundled one, or connecting.
    Connecting,
    Ready,
    /// A core that had been reachable is gone; the bundled one is started after the grace
    /// period (`retryInMs`).
    Waiting,
    /// A core answers but Yuzora does not use it (`reason`); it is left untouched.
    Incompatible,
    /// The bundled core could not be started, or the endpoint cannot be used (`reason`).
    StartFailed,
    /// Something is at the endpoint but did not answer sensibly (`reason`).
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreSource {
    /// A core that was already running: left alone.
    External,
    /// Started from the bundled binary by Yuzora, in this run or an earlier one (K9).
    Bundled,
}

/// A stable `code` (the host's `Refusal`, `StartError`, `ClientError` or `PathError` code)
/// and the host's English text for logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoreReason {
    pub code: String,
    pub detail: String,
}

impl CoreReason {
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> CoreReason {
        CoreReason {
            code: code.into(),
            detail: detail.into(),
        }
    }

    pub fn from_refusal(refusal: &Refusal) -> CoreReason {
        CoreReason::new(refusal.code(), refusal.to_string())
    }

    pub fn from_start_error(error: &StartError) -> CoreReason {
        CoreReason::new(error.code(), error.to_string())
    }

    pub fn from_client_error(error: &ClientError) -> CoreReason {
        CoreReason::new(error.code(), error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    /// Counts up with every change, so a late event can be told from a newer one.
    pub seq: u64,
    pub enabled: bool,
    /// The session this build uses (`yuzora-dev` in debug builds).
    pub session: String,
    pub state: CoreState,
    /// Which connection the events belong to: it grows with every session connected, and
    /// everything attached or subscribed on an earlier one is gone.
    pub epoch: u64,
    /// The core's version (also when it is incompatible).
    pub version: Option<String>,
    pub source: Option<CoreSource>,
    pub bundled_version: String,
    /// Methods this core lacks that the UI must hide.
    pub optional_missing: Vec<String>,
    pub reason: Option<CoreReason>,
    pub retry_in_ms: Option<u64>,
    pub socket: Option<String>,
    /// Why the start record could not be written (a later restart could not recognize the
    /// core as ours).
    pub record_error: Option<String>,
}

/// Everything about a status except what the service keeps: `seq`, `enabled`, `session`
/// and `epoch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub state: CoreState,
    pub version: Option<String>,
    pub source: Option<CoreSource>,
    pub optional_missing: Vec<String>,
    pub reason: Option<CoreReason>,
    pub retry_in_ms: Option<u64>,
    pub socket: Option<String>,
    pub record_error: Option<String>,
}

impl Draft {
    pub fn new(state: CoreState) -> Draft {
        Draft {
            state,
            version: None,
            source: None,
            optional_missing: Vec::new(),
            reason: None,
            retry_in_ms: None,
            socket: None,
            record_error: None,
        }
    }

    pub fn with_socket(mut self, socket: &std::path::Path) -> Draft {
        self.socket = Some(socket.to_string_lossy().into_owned());
        self
    }

    pub fn with_reason(mut self, reason: CoreReason) -> Draft {
        self.reason = Some(reason);
        self
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Draft {
        self.version = Some(version.into());
        self
    }

    pub fn with_retry(mut self, retry: std::time::Duration) -> Draft {
        self.retry_in_ms = Some(u64::try_from(retry.as_millis()).unwrap_or(u64::MAX));
        self
    }

    /// `source` is what the supervisor knows; `bundled_earlier` is the K9 verdict for a core
    /// the supervisor met as an already-running one.
    pub fn ready(ready: &Ready, bundled_earlier: bool) -> Draft {
        let source = match (&ready.origin, bundled_earlier) {
            (Origin::StartedByYuzora(_), _) | (Origin::External, true) => CoreSource::Bundled,
            (Origin::External, false) => CoreSource::External,
        };
        let mut draft = Draft::new(CoreState::Ready).with_version(ready.hello.version.clone());
        draft.source = Some(source);
        draft.optional_missing = ready
            .optional_missing
            .iter()
            .map(|m| m.to_string())
            .collect();
        draft.record_error = ready.record_error.clone();
        draft
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use yuzora_host::herdr_core::caps::{Hello, Relation};
    use yuzora_host::herdr_core::startup::StartRecord;

    fn ready(origin: Origin) -> Ready {
        Ready {
            hello: Hello {
                version: "0.1.14".into(),
                generation: 1,
                methods: BTreeSet::new(),
                features: BTreeSet::new(),
                core_id: "c".into(),
                framed: false,
            },
            relation: Relation::Same,
            optional_missing: vec!["terminal.keep"],
            origin,
            record_error: Some("disk full".into()),
        }
    }

    fn record() -> StartRecord {
        StartRecord {
            v: 1,
            pid: 7,
            started_unix_ms: 1,
            exe: "/x".into(),
            version: "0.1.14".into(),
            session: "yuzora".into(),
            socket: "/s".into(),
        }
    }

    #[test]
    fn herdr_core_status_source_follows_the_origin_and_the_k9_verdict() {
        let started = Draft::ready(&ready(Origin::StartedByYuzora(record())), false);
        assert_eq!(started.source, Some(CoreSource::Bundled));
        let external = Draft::ready(&ready(Origin::External), false);
        assert_eq!(external.source, Some(CoreSource::External));
        let earlier_run = Draft::ready(&ready(Origin::External), true);
        assert_eq!(earlier_run.source, Some(CoreSource::Bundled));
        assert_eq!(external.version.as_deref(), Some("0.1.14"));
        assert_eq!(external.optional_missing, ["terminal.keep"]);
        assert_eq!(external.record_error.as_deref(), Some("disk full"));
    }

    #[test]
    fn herdr_core_status_serializes_snake_case_states_and_camel_case_fields() {
        let status = CoreStatus {
            seq: 3,
            enabled: true,
            session: "yuzora".into(),
            state: CoreState::StartFailed,
            epoch: 2,
            version: None,
            source: Some(CoreSource::External),
            bundled_version: "0.1.14".into(),
            optional_missing: vec![],
            reason: Some(CoreReason::new("spawn", "no")),
            retry_in_ms: Some(1500),
            socket: None,
            record_error: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "start_failed");
        assert_eq!(json["source"], "external");
        assert_eq!(json["retryInMs"], 1500);
        assert_eq!(json["bundledVersion"], "0.1.14");
        assert_eq!(json["optionalMissing"], serde_json::json!([]));
        assert_eq!(json["reason"]["code"], "spawn");
        for (state, name) in [
            (CoreState::Disabled, "disabled"),
            (CoreState::Connecting, "connecting"),
            (CoreState::Ready, "ready"),
            (CoreState::Waiting, "waiting"),
            (CoreState::Incompatible, "incompatible"),
            (CoreState::Unreachable, "unreachable"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
        }
        assert_eq!(STATUS_EVENT, "herdr-core:status");
    }
}
