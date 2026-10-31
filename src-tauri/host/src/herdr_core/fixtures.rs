//! Test access to `tests/fixtures/herdr-core/` (recorded against the pinned core in an
//! isolated session; see that directory's README.md).

use std::path::PathBuf;

use base64::Engine as _;
use serde_json::Value;

use super::frame::{self, Dir, Frame};
use super::rpc::RpcError;

pub const BUNDLED: &str = "0.1.14";
pub const OLDER: &str = "0.1.11";
pub const VERSIONS: &[&str] = &[BUNDLED, OLDER];

pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/herdr-core")
}

pub fn json(rel: &str) -> Value {
    let path = dir().join(rel);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// One recorded session (`v<BUNDLED>/<name>`): a header line, then one line per message or
/// frame.
pub struct Recorded {
    pub lines: Vec<Value>,
}

pub fn recorded(name: &str) -> Recorded {
    let path = dir().join(format!("v{BUNDLED}")).join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut it = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("ndjson line"));
    let header = it.next().expect("header line");
    assert_eq!(
        header["version"], BUNDLED,
        "{name} recorded against another version"
    );
    Recorded {
        lines: it.collect(),
    }
}

impl Recorded {
    fn msgs(&self) -> impl Iterator<Item = (&Value, &Value)> {
        // (line, JSON-RPC message) for plain lines and channel-0 framed DATA.
        self.lines
            .iter()
            .filter_map(|l| l.get("msg").map(|m| (l, m)))
    }

    pub fn subscribe_result(&self) -> Option<Value> {
        let id = self
            .msgs()
            .find(|(l, m)| l["dir"] == "out" && m["method"] == "events.subscribe")?
            .1["id"]
            .clone();
        self.msgs()
            .find(|(l, m)| l["dir"] == "in" && m["id"] == id && m.get("result").is_some())
            .map(|(_, m)| m["result"].clone())
    }

    /// Notifications (`event`, `heartbeat`, `missed`) in arrival order, from plain lines and
    /// from framed event channels (recorded as `msg` too).
    pub fn notifications(&self) -> Vec<(String, Value)> {
        self.msgs()
            .filter(|(l, m)| {
                l["dir"] == "in"
                    && m.get("id").is_none_or(Value::is_null)
                    && m.get("method").is_some()
            })
            .map(|(_, m)| {
                (
                    m["method"].as_str().unwrap().to_string(),
                    m["params"].clone(),
                )
            })
            .collect()
    }

    pub fn final_list(&self) -> Option<Value> {
        self.notes()
            .into_iter()
            .rev()
            .find_map(|n| n.get("final_list").cloned())
    }

    pub fn notes(&self) -> Vec<Value> {
        self.lines
            .iter()
            .filter_map(|l| l.get("note").cloned())
            .collect()
    }

    pub fn errors(&self) -> Vec<RpcError> {
        self.msgs()
            .filter(|(l, m)| l["dir"] == "in" && m.get("error").is_some())
            .map(|(_, m)| RpcError::from_json(&m["error"]))
            .collect()
    }

    /// (request, response) pairs on one connection, in request order.
    pub fn request_response_pairs_for(&self, conn: &str) -> Vec<(Value, Value)> {
        let mine: Vec<(&Value, &Value)> = self.msgs().filter(|(l, _)| l["conn"] == conn).collect();
        mine.iter()
            .filter(|(l, m)| l["dir"] == "out" && m.get("method").is_some())
            .filter_map(|(_, req)| {
                let resp = mine.iter().find(|(l, m)| {
                    l["dir"] == "in" && m["id"] == req["id"] && m.get("method").is_none()
                })?;
                Some(((*req).clone(), resp.1.clone()))
            })
            .collect()
    }

    /// Framed frames of one connection; elided payloads (only `len` kept) become zero bytes.
    pub fn frames(&self, conn: &str) -> Vec<(Dir, Frame)> {
        self.lines
            .iter()
            .filter(|l| l["conn"] == conn && l["mode"] == "framed")
            .map(|l| {
                let dir = if l["dir"] == "in" { Dir::In } else { Dir::Out };
                let channel = l["channel"].as_u64().unwrap() as u32;
                let kind = frame::kind_from_name(l["kind"].as_str().unwrap()).expect("kind");
                let payload = if let Some(b) = l.get("b64").and_then(Value::as_str) {
                    base64::engine::general_purpose::STANDARD
                        .decode(b)
                        .expect("b64")
                } else if let Some(m) = l.get("msg") {
                    // Channel-0 JSON is kept decoded; its re-serialised length may differ.
                    return (
                        dir,
                        Frame {
                            channel,
                            kind,
                            payload: m.to_string().into_bytes(),
                        },
                    );
                } else {
                    vec![0; l["len"].as_u64().unwrap() as usize]
                };
                assert_eq!(
                    payload.len() as u64,
                    l["len"].as_u64().unwrap(),
                    "len of {l}"
                );
                (
                    dir,
                    Frame {
                        channel,
                        kind,
                        payload,
                    },
                )
            })
            .collect()
    }
}
