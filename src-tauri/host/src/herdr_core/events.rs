//! Event snapshot + `seq` apply rules (upstream `schema/README.md` "Events"):
//! `events.subscribe` gives `{seq, terminals}`; each `event` with a `seq` not above the current one
//! is dropped; `missed` replaces the snapshot; `seq` restarts with every core process, so a
//! reconnect always starts from a fresh snapshot (and a fresh read of documents).

use std::collections::BTreeMap;

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A kept terminal got a new shell: attach again.
    Reattach(String),
    /// Its attach streams end (lost or finished); for `lost`, wait for `Reattach`.
    StreamsEnd {
        terminal: String,
        lost: bool,
    },
    Removed(String),
    Added(String),
    StatusChanged(String),
    /// A fact of a live terminal changed that has no effect of its own: its `title`, `turn`,
    /// `size`, `foreground`, `keep` or `labels` (while it still matches the subscription's
    /// label filter). The event store is already updated; read the record.
    Updated(String),
    /// A document changed (`machine` set for another machine's): call `doc.get`.
    DocChanged {
        name: String,
        version: u64,
        machine: Option<String>,
    },
    /// The snapshot was replaced (`missed`): refetch every watched document.
    Resnapshot,
    /// Something only a fresh `events.subscribe` can fix (gap on an unfiltered stream, unknown
    /// terminal, seq going backwards).
    NeedResnapshot(String),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub applied: usize,
    pub stale_dropped: usize,
    pub gaps: usize,
    pub missed: usize,
    pub heartbeats: usize,
    pub unknown_types: usize,
}

#[derive(Debug, Clone)]
pub struct EventStore {
    pub seq: u64,
    pub terminals: BTreeMap<String, Value>,
    /// Subscribed with a filter: gaps in `seq` are normal (other terminals' events).
    pub filtered: bool,
    /// Label filter used for the subscription (a terminal whose labels stop matching leaves).
    pub label_filter: BTreeMap<String, String>,
    pub stats: Stats,
    pub last_heartbeat_seq: Option<u64>,
}

impl EventStore {
    /// From an `events.subscribe` result (or a `missed` params object: same shape).
    pub fn from_snapshot(
        snapshot: &Value,
        label_filter: BTreeMap<String, String>,
        filtered: bool,
    ) -> Result<EventStore, String> {
        let mut s = EventStore {
            seq: 0,
            terminals: BTreeMap::new(),
            filtered: filtered || !label_filter.is_empty(),
            label_filter,
            stats: Stats::default(),
            last_heartbeat_seq: None,
        };
        s.replace(snapshot)?;
        Ok(s)
    }

    fn replace(&mut self, snapshot: &Value) -> Result<(), String> {
        self.seq = snapshot
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or("snapshot without seq")?;
        self.terminals.clear();
        for t in snapshot
            .get("terminals")
            .and_then(Value::as_array)
            .ok_or("snapshot without terminals")?
        {
            let addr = t
                .get("address")
                .and_then(Value::as_str)
                .ok_or("terminal without address")?;
            self.terminals.insert(addr.to_string(), t.clone());
        }
        Ok(())
    }

    /// One notification (`event`, `heartbeat`, `missed`).
    pub fn apply(&mut self, method: &str, params: &Value) -> Vec<Effect> {
        match method {
            "heartbeat" => {
                self.stats.heartbeats += 1;
                let seq = params.get("seq").and_then(Value::as_u64).unwrap_or(0);
                self.last_heartbeat_seq = Some(seq);
                // A heartbeat is liveness only; it never advances `seq` (in plain mode it can
                // overtake queued events). Going backwards means another core process.
                if seq < self.seq {
                    return vec![Effect::NeedResnapshot(format!(
                        "heartbeat seq {seq} below applied {}",
                        self.seq
                    ))];
                }
                vec![]
            }
            "missed" => {
                self.stats.missed += 1;
                match self.replace(params) {
                    Ok(()) => vec![Effect::Resnapshot],
                    Err(e) => vec![Effect::NeedResnapshot(e)],
                }
            }
            "event" => self.apply_event(params),
            _ => {
                self.stats.unknown_types += 1;
                vec![]
            }
        }
    }

    fn apply_event(&mut self, p: &Value) -> Vec<Effect> {
        let Some(seq) = p.get("seq").and_then(Value::as_u64) else {
            return vec![Effect::NeedResnapshot("event without seq".into())];
        };
        if seq <= self.seq {
            self.stats.stale_dropped += 1;
            return vec![];
        }
        let mut out = Vec::new();
        if seq != self.seq + 1 {
            self.stats.gaps += 1;
            if !self.filtered {
                out.push(Effect::NeedResnapshot(format!(
                    "gap: {} -> {seq} on an unfiltered stream",
                    self.seq
                )));
            }
        }
        self.seq = seq;
        self.stats.applied += 1;
        let kind = p.get("type").and_then(Value::as_str).unwrap_or_default();
        let data = p.get("data").cloned().unwrap_or(Value::Null);
        if kind == "doc" {
            out.push(Effect::DocChanged {
                name: data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                version: data.get("version").and_then(Value::as_u64).unwrap_or(0),
                machine: p.get("machine").and_then(Value::as_str).map(String::from),
            });
            return out;
        }
        let Some(addr) = p.get("terminal").and_then(Value::as_str).map(String::from) else {
            self.stats.unknown_types += 1;
            return out;
        };
        match kind {
            "created" => {
                self.terminals.insert(addr.clone(), data);
                out.push(Effect::Added(addr));
                return out;
            }
            "restarted" => {
                self.terminals.insert(addr.clone(), data);
                out.push(Effect::Reattach(addr));
                return out;
            }
            "removed" => {
                if self.terminals.remove(&addr).is_none() && !self.filtered {
                    out.push(Effect::NeedResnapshot(format!("removed unknown {addr}")));
                }
                out.push(Effect::Removed(addr));
                return out;
            }
            "labels" => {
                if let Some(record) = data.get("record") {
                    self.terminals.insert(addr.clone(), record.clone());
                    out.push(Effect::Added(addr.clone()));
                }
            }
            _ => {}
        }
        let Some(t) = self.terminals.get_mut(&addr) else {
            if !self.filtered {
                out.push(Effect::NeedResnapshot(format!("{kind} for unknown {addr}")));
            }
            return out;
        };
        let set = |t: &mut Value, keys: &[&str], data: &Value| {
            for k in keys {
                t[*k] = data.get(*k).cloned().unwrap_or(Value::Null);
            }
        };
        match kind {
            "lost" => {
                set(t, &["reason", "exit_code", "signal", "finished_ms"], &data);
                t["pid"] = Value::Null;
                t["foreground"] = Value::Null;
                out.push(Effect::StreamsEnd {
                    terminal: addr.clone(),
                    lost: true,
                });
            }
            "keep" => {
                set(t, &["keep"], &data);
                out.push(Effect::Updated(addr));
            }
            "finished" => {
                t["state"] = json!("finished");
                set(t, &["reason", "exit_code", "signal", "finished_ms"], &data);
                t["foreground"] = Value::Null;
                out.push(Effect::StreamsEnd {
                    terminal: addr.clone(),
                    lost: false,
                });
            }
            "status" => {
                set(t, &["status", "records", "reported"], &data);
                out.push(Effect::StatusChanged(addr.clone()));
            }
            "turn" => {
                set(t, &["turn"], &data);
                out.push(Effect::Updated(addr));
            }
            "title" => {
                set(t, &["title"], &data);
                out.push(Effect::Updated(addr));
            }
            "labels" => {
                set(t, &["labels"], &data);
                let still = self
                    .label_filter
                    .iter()
                    .all(|(k, v)| t["labels"][k] == v.as_str());
                if !still {
                    self.terminals.remove(&addr);
                    out.push(Effect::Removed(addr));
                } else if !out.contains(&Effect::Added(addr.clone())) {
                    // A record that entered the stream above was reported as `Added` already.
                    out.push(Effect::Updated(addr));
                }
            }
            "size" => {
                set(t, &["cols", "rows"], &data);
                out.push(Effect::Updated(addr));
            }
            "foreground" => {
                t["foreground"] = data;
                out.push(Effect::Updated(addr));
            }
            _ => self.stats.unknown_types += 1,
        }
        out
    }
}

/// Fields compared between a replayed store and the core's own `terminal.list` at the end of a
/// recording (`pid`/`foreground` are sampled by the core about twice a second: skipped).
pub const COMPARED_FIELDS: &[&str] = &[
    "address",
    "state",
    "status",
    "records",
    "reported",
    "keep",
    "title",
    "cols",
    "rows",
    "labels",
    "reason",
    "exit_code",
    "turn",
    "parent",
];

pub fn project(t: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in COMPARED_FIELDS {
        let v = t.get(*k).cloned().unwrap_or(Value::Null);
        // The core omits empty/default fields in some records: normalise.
        let v = match (*k, v) {
            ("records", Value::Null) => json!({}),
            ("reported", Value::Null) | ("keep", Value::Null) => json!(false),
            ("turn", Value::Null) => json!(0),
            ("title", Value::Null) => json!(""),
            (_, v) => v,
        };
        out.insert(k.to_string(), v);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr_core::fixtures::{self, Recorded};

    /// Replays a recorded plain events stream; returns the store and the effects seen.
    fn replay(name: &str) -> (EventStore, Vec<Effect>, Recorded) {
        let rec = fixtures::recorded(name);
        let sub = rec.subscribe_result().expect("subscribe result in fixture");
        let mut store = EventStore::from_snapshot(&sub, BTreeMap::new(), false).unwrap();
        let mut effects = Vec::new();
        for (method, params) in rec.notifications() {
            effects.extend(store.apply(&method, &params));
        }
        (store, effects, rec)
    }

    fn assert_matches_final_list(store: &EventStore, rec: &Recorded) {
        let list = rec.final_list().expect("final terminal.list in fixture");
        let want: BTreeMap<String, Value> = list["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["address"].as_str().unwrap().to_string(), project(t)))
            .collect();
        let got: BTreeMap<String, Value> = store
            .terminals
            .iter()
            .map(|(k, t)| (k.clone(), project(t)))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn lifecycle_stream_rebuilds_the_cores_own_list() {
        let (store, effects, rec) = replay("events-lifecycle.ndjson");
        assert_matches_final_list(&store, &rec);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::NeedResnapshot(_))),
            "{effects:?}"
        );
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::StatusChanged(_))));
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::StreamsEnd { lost: false, .. })));
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::DocChanged { name, .. } if name == "yuzora")));
        assert!(
            store.stats.heartbeats >= 1,
            "recorded with a short HC_HEARTBEAT_MS"
        );
        assert_eq!(store.stats.gaps, 0);
    }

    #[test]
    fn keep_lost_restarted_asks_for_reattach() {
        let (store, effects, rec) = replay("events-keep.ndjson");
        assert_matches_final_list(&store, &rec);
        let lost = effects
            .iter()
            .position(|e| matches!(e, Effect::StreamsEnd { lost: true, .. }))
            .expect("lost");
        let again = effects
            .iter()
            .position(|e| matches!(e, Effect::Reattach(_)))
            .expect("restarted");
        assert!(lost < again);
    }

    #[test]
    fn missed_replaces_the_snapshot_and_drops_older_events() {
        let (store, effects, rec) = replay("events-missed.ndjson");
        assert!(
            store.stats.missed >= 1,
            "fixture holds a missed notification"
        );
        assert!(effects.contains(&Effect::Resnapshot));
        assert_matches_final_list(&store, &rec);
        // Re-applying any event at or below the snapshot seq is a no-op.
        let before = store.terminals.clone();
        let mut again = store.clone();
        for (method, params) in rec
            .notifications()
            .into_iter()
            .filter(|(m, _)| m == "event")
        {
            if params["seq"].as_u64().unwrap() <= store.seq {
                again.apply(&method, &params);
            }
        }
        assert_eq!(again.terminals, before);
    }

    #[test]
    fn seq_restarts_with_the_core_so_old_state_must_not_be_reused() {
        let before = fixtures::recorded("events-restart-before.ndjson");
        let after = fixtures::recorded("events-restart-after.ndjson");
        let old_seq = {
            let s = before.subscribe_result().unwrap();
            let mut st = EventStore::from_snapshot(&s, BTreeMap::new(), false).unwrap();
            for (m, p) in before.notifications() {
                st.apply(&m, &p);
            }
            st.seq
        };
        let new_sub = after.subscribe_result().unwrap();
        let new_seq = new_sub["seq"].as_u64().unwrap();
        assert!(
            new_seq < old_seq,
            "core restart resets seq ({old_seq} -> {new_seq})"
        );
        // Wrong: keep the old store and its seq: every new event looks stale.
        let mut stale = EventStore::from_snapshot(
            &json!({"seq": old_seq, "terminals": []}),
            BTreeMap::new(),
            false,
        )
        .unwrap();
        let mut dropped = 0;
        for (m, p) in after.notifications() {
            let before = stale.stats.stale_dropped;
            stale.apply(&m, &p);
            dropped += stale.stats.stale_dropped - before;
        }
        assert!(dropped > 0, "old seq would drop new events");
        // Right: fresh snapshot after reconnect.
        let mut fresh = EventStore::from_snapshot(&new_sub, BTreeMap::new(), false).unwrap();
        for (m, p) in after.notifications() {
            fresh.apply(&m, &p);
        }
        assert_eq!(fresh.stats.stale_dropped, 0);
        assert_matches_final_list(&fresh, &after);
    }

    #[test]
    fn title_turn_size_foreground_and_keep_each_report_an_update() {
        let mut s = EventStore::from_snapshot(
            &json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "title": "a"}]}),
            BTreeMap::new(),
            false,
        )
        .unwrap();
        let events = [
            ("title", json!({"title": "b"})),
            ("turn", json!({"turn": 3, "state": "done"})),
            ("size", json!({"cols": 90, "rows": 25})),
            (
                "foreground",
                json!({"pid": 5, "name": "vim", "argv": ["vim"], "cwd": "/a"}),
            ),
            ("keep", json!({"keep": true})),
        ];
        for (i, (kind, data)) in events.into_iter().enumerate() {
            let seq = i as u64 + 1;
            let got = s.apply(
                "event",
                &json!({"seq": seq, "type": kind, "terminal": "t1", "data": data}),
            );
            assert_eq!(got, vec![Effect::Updated("t1".into())], "{kind}");
        }
        // The store followed every one of them.
        let t = &s.terminals["t1"];
        assert_eq!(
            (t["title"].as_str(), t["turn"].as_u64(), t["cols"].as_u64()),
            (Some("b"), Some(3), Some(90))
        );
        assert_eq!(
            (t["foreground"]["name"].as_str(), t["keep"].as_bool()),
            (Some("vim"), Some(true))
        );
        // A terminal the store does not know is a resnapshot, not an update.
        let got = s.apply(
            "event",
            &json!({"seq": 6, "type": "title", "terminal": "t9", "data": {"title": "x"}}),
        );
        assert!(matches!(&got[..], [Effect::NeedResnapshot(_)]), "{got:?}");
        // And an event that is dropped as stale reports nothing.
        assert!(s
            .apply(
                "event",
                &json!({"seq": 6, "type": "title", "terminal": "t1", "data": {"title": "late"}})
            )
            .is_empty());
    }

    #[test]
    fn a_labels_change_reports_an_update_while_the_terminal_stays_in_the_stream() {
        let labels = |seq: u64, terminal: &str, labels: Value| {
            json!({"seq": seq, "type": "labels", "terminal": terminal,
                "data": {"labels": labels}})
        };
        // Unfiltered: every change is the observer's to hear about.
        let mut s = EventStore::from_snapshot(
            &json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "labels": {}}]}),
            BTreeMap::new(),
            false,
        )
        .unwrap();
        let got = s.apply("event", &labels(1, "t1", json!({"space": "a"})));
        assert_eq!(got, vec![Effect::Updated("t1".into())]);
        assert_eq!(s.terminals["t1"]["labels"], json!({"space": "a"}));

        // Filtered: a change that still matches is an update, one that stops matching is a
        // removal, and a record that enters the stream is reported once, as added.
        let mut f = EventStore::from_snapshot(
            &json!({"seq": 0, "terminals": [{"address": "t2", "labels": {"space": "a"}}]}),
            BTreeMap::from([("space".to_string(), "a".to_string())]),
            true,
        )
        .unwrap();
        let got = f.apply(
            "event",
            &labels(1, "t2", json!({"space": "a", "role": "build"})),
        );
        assert_eq!(got, vec![Effect::Updated("t2".into())]);
        assert_eq!(f.terminals["t2"]["labels"]["role"], "build");
        let got = f.apply(
            "event",
            &json!({"seq": 2, "type": "labels", "terminal": "t3", "data": {
                "labels": {"space": "a"},
                "record": {"address": "t3", "state": "live", "labels": {"space": "a"}}}}),
        );
        assert_eq!(got, vec![Effect::Added("t3".into())]);
        let got = f.apply("event", &labels(3, "t2", json!({"space": "b"})));
        assert_eq!(got, vec![Effect::Removed("t2".into())]);
        assert!(!f.terminals.contains_key("t2"));
    }

    #[test]
    fn rules_on_synthetic_events() {
        let mut s = EventStore::from_snapshot(&json!({"seq": 5, "terminals": [{"address": "t1", "state": "live", "pid": 9, "foreground": {"pid": 9}}]}), BTreeMap::new(), false).unwrap();
        assert!(s
            .apply(
                "event",
                &json!({"seq": 5, "type": "title", "terminal": "t1", "data": {"title": "x"}})
            )
            .is_empty());
        assert_eq!(s.stats.stale_dropped, 1);
        let e = s.apply(
            "event",
            &json!({"seq": 7, "type": "title", "terminal": "t1", "data": {"title": "y"}}),
        );
        assert!(matches!(&e[0], Effect::NeedResnapshot(why) if why.contains("gap")));
        s.apply("event", &json!({"seq": 8, "type": "lost", "terminal": "t1", "data": {"reason": "signaled", "exit_code": null, "signal": 9, "finished_ms": null}}));
        assert_eq!(s.terminals["t1"]["state"], "live");
        assert!(s.terminals["t1"]["pid"].is_null() && s.terminals["t1"]["foreground"].is_null());
        assert!(matches!(
            &s.apply("heartbeat", &json!({"seq": 3}))[0],
            Effect::NeedResnapshot(_)
        ));
        let mut f = EventStore::from_snapshot(
            &json!({"seq": 1, "terminals": [{"address": "t2", "labels": {"space": "a"}}]}),
            BTreeMap::from([("space".to_string(), "a".to_string())]),
            true,
        )
        .unwrap();
        let e = f.apply("event", &json!({"seq": 9, "type": "labels", "terminal": "t2", "data": {"labels": {"space": "b"}}}));
        assert_eq!(e, vec![Effect::Removed("t2".into())]);
        assert_eq!(f.stats.gaps, 1, "gaps are normal on a filtered stream");
    }
}
