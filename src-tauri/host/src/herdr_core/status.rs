//! Agent status for the Agents sidebar and Attention: only what the program reported through
//! OSC 7501 (`status`, `records`, `reported` on the terminal record). Never inferred.
//!
//! This module returns enums and the program's own words (`app`, `title`, `msg`); every UI
//! label is the frontend's (i18n).

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working {
        progress: Option<u8>,
    },
    Blocked {
        kind: Option<String>,
    },
    Done,
    Error,
    /// A word outside the five known ones, as reported.
    Other(String),
    /// Reported before, root record cleared now.
    Cleared,
    /// Never reported (`reported: false`): no status, no Attention.
    NoStatus,
}

/// Mirrors the HERDR path's Attention kinds that a 7501 report can produce. `unknown` is never
/// produced here: a program that does not speak 7501 has no status, not an unknown one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    Blocked,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentView {
    pub state: AgentState,
    pub app: String,
    pub title: String,
    pub msg: String,
    /// Other 7501 records by key (programs reporting on several things).
    pub children_records: usize,
}

impl AgentView {
    pub fn from_terminal(t: &Value) -> AgentView {
        let reported = t.get("reported").and_then(Value::as_bool).unwrap_or(false);
        let status = t.get("status").filter(|s| !s.is_null());
        let text = |k: &str| {
            status
                .and_then(|s| s.get(k))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let state = match status {
            None if reported => AgentState::Cleared,
            None => AgentState::NoStatus,
            Some(s) => match s.get("state").and_then(Value::as_str).unwrap_or_default() {
                "idle" => AgentState::Idle,
                "working" => AgentState::Working {
                    progress: s
                        .get("progress")
                        .and_then(Value::as_u64)
                        .map(|p| p.min(100) as u8),
                },
                "blocked" => AgentState::Blocked {
                    kind: s.get("kind").and_then(Value::as_str).map(String::from),
                },
                "done" => AgentState::Done,
                "error" => AgentState::Error,
                other => AgentState::Other(other.to_string()),
            },
        };
        AgentView {
            state,
            app: text("app"),
            title: text("title"),
            msg: text("msg"),
            children_records: t
                .get("records")
                .and_then(Value::as_object)
                .map_or(0, |r| r.len()),
        }
    }

    pub fn attention(&self) -> Option<Attention> {
        match self.state {
            AgentState::Blocked { .. } => Some(Attention::Blocked),
            AgentState::Done => Some(Attention::Done),
            AgentState::Error => Some(Attention::Error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr_core::fixtures;
    use serde_json::json;

    #[test]
    fn recorded_osc_7501_report_reaches_the_record() {
        // `printf '\e]7501;state=blocked:app=my-agent\e\\'` in an isolated core, then terminal.get.
        let rec = fixtures::recorded("status-7501.ndjson");
        let events: Vec<Value> = rec
            .notifications()
            .into_iter()
            .filter(|(m, p)| m == "event" && p["type"] == "status")
            .map(|(_, p)| p)
            .collect();
        let blocked = events
            .iter()
            .find(|p| p["data"]["status"]["state"] == "blocked")
            .expect("blocked event");
        assert_eq!(blocked["data"]["status"]["app"], "my-agent");
        assert_eq!(blocked["data"]["reported"], true);
        let view = AgentView::from_terminal(
            &json!({"status": blocked["data"]["status"], "reported": true}),
        );
        assert_eq!(view.state, AgentState::Blocked { kind: None });
        assert_eq!(view.attention(), Some(Attention::Blocked));
        assert_eq!(view.app, "my-agent");
        // The program that never reported has no status.
        let silent = rec.final_list().unwrap()["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "silent")
            .cloned()
            .expect("silent terminal recorded");
        let view = AgentView::from_terminal(&silent);
        assert_eq!(view.attention(), None);
        assert_eq!(view.state, AgentState::NoStatus);
        // A kind survives the round trip.
        let rich = events
            .iter()
            .find(|p| p["data"]["status"]["kind"] == "permission")
            .expect("blocked kind=permission");
        assert_eq!(
            AgentView::from_terminal(&json!({"status": rich["data"]["status"], "reported": true}))
                .state,
            AgentState::Blocked {
                kind: Some("permission".into())
            }
        );
        // Clearing (empty 7501) leaves `reported: true`, status null.
        let cleared = events
            .iter()
            .find(|p| p["data"]["status"].is_null())
            .expect("cleared status event");
        assert_eq!(
            AgentView::from_terminal(
                &json!({"status": null, "reported": cleared["data"]["reported"]})
            )
            .state,
            AgentState::Cleared
        );
    }

    #[test]
    fn mapping() {
        let view = |s: Value| AgentView::from_terminal(&json!({"status": s, "reported": true}));
        assert_eq!(
            view(json!({"state": "working", "progress": 40})).state,
            AgentState::Working { progress: Some(40) }
        );
        assert_eq!(
            view(json!({"state": "working", "progress": 4000})).state,
            AgentState::Working {
                progress: Some(100)
            }
        );
        assert_eq!(
            view(json!({"state": "done"})).attention(),
            Some(Attention::Done)
        );
        assert_eq!(
            view(json!({"state": "error"})).attention(),
            Some(Attention::Error)
        );
        assert_eq!(view(json!({"state": "idle"})).attention(), None);
        assert_eq!(
            view(json!({"state": "thinking"})).state,
            AgentState::Other("thinking".into())
        );
        let child_records = AgentView::from_terminal(&json!({
            "status": {"state": "working", "title": "build"},
            "records": {"a": {"state": "idle"}, "b": {"state": "done"}},
            "reported": true
        }));
        assert_eq!(child_records.children_records, 2);
        assert_eq!(child_records.title, "build");
    }
}
