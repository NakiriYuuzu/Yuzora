//! JSON the frontend receives: the events channel (`herdr_core_events_subscribe`) and the
//! control messages that share an attach's byte channel.
use serde::Serialize;
use serde_json::Value;
use yuzora_host::herdr_core::client::{AttachEnd, LayoutSnapshot};
use yuzora_host::herdr_core::docs::LayoutState;
use yuzora_host::herdr_core::events::Effect;
use yuzora_host::herdr_core::frame::CloseReason;

/// Everything on the events channel carries the `epoch` of the connection it belongs to
/// (see `CoreStatus::epoch`): after a reconnect, anything with an older epoch is stale.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CoreEvent {
    /// The core's terminals as the event stream describes them now: sent on subscribing,
    /// on every (re)connect and after `missed`. Upsert each record by `address`, and drop
    /// the ones that are not listed.
    Snapshot { epoch: u64, terminals: Vec<Value> },
    /// What changed since the last snapshot or effects message.
    Effects {
        epoch: u64,
        effects: Vec<EffectView>,
    },
    /// The layout document was read again (on connect, after `missed`, on a `doc` event for
    /// it); only sent when its version differs from the last one the session delivered.
    ///
    /// The same version can still arrive twice (a new subscription's initial layout, then the
    /// broadcast that raced it), so the receiver drops a `Layout` whose `version` is not greater
    /// than the one it already knows for the same `epoch`. A newer `epoch` replaces the older
    /// one whatever the versions say. Per subscriber, the initial `Snapshot` and `Layout` come
    /// before every other event.
    Layout {
        epoch: u64,
        version: u64,
        state: LayoutState,
    },
}

impl CoreEvent {
    pub fn layout(epoch: u64, layout: &LayoutSnapshot) -> CoreEvent {
        CoreEvent::Layout {
            epoch,
            version: layout.version,
            state: layout.state.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EffectView {
    /// `record` is the terminal as the event store has it (when the connection is installed).
    Added {
        terminal: String,
        record: Option<Value>,
    },
    /// A kept terminal got a new shell: attach again.
    Reattach {
        terminal: String,
        record: Option<Value>,
    },
    StatusChanged {
        terminal: String,
        record: Option<Value>,
    },
    /// Its `title`, `turn`, `size`, `foreground` or `keep` changed: replace the record.
    Updated {
        terminal: String,
        record: Option<Value>,
    },
    Removed {
        terminal: String,
    },
    /// Its attach streams end (also reported by each attach's own `closed` message).
    StreamsEnd {
        terminal: String,
        lost: bool,
    },
    DocChanged {
        name: String,
        version: u64,
        machine: Option<String>,
    },
    /// The core replaced its snapshot (`missed`); a `snapshot` event follows.
    Resnapshot,
    /// The local view can no longer be trusted: the connection is ending and will be redone.
    NeedResnapshot {
        reason: String,
    },
}

/// Whether the view of this effect carries the terminal's record.
pub fn needs_record(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::Added(_) | Effect::Reattach(_) | Effect::StatusChanged(_) | Effect::Updated(_)
    )
}

fn record_of(records: Option<&[Value]>, address: &str) -> Option<Value> {
    records?
        .iter()
        .find(|record| record.get("address").and_then(Value::as_str) == Some(address))
        .cloned()
}

/// `records`: the event store's terminals, when the caller has them (only needed if
/// [`needs_record`] holds for some effect).
pub fn view_effects(effects: &[Effect], records: Option<&[Value]>) -> Vec<EffectView> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::Added(terminal) => EffectView::Added {
                record: record_of(records, terminal),
                terminal: terminal.clone(),
            },
            Effect::Reattach(terminal) => EffectView::Reattach {
                record: record_of(records, terminal),
                terminal: terminal.clone(),
            },
            Effect::StatusChanged(terminal) => EffectView::StatusChanged {
                record: record_of(records, terminal),
                terminal: terminal.clone(),
            },
            Effect::Updated(terminal) => EffectView::Updated {
                record: record_of(records, terminal),
                terminal: terminal.clone(),
            },
            Effect::Removed(terminal) => EffectView::Removed {
                terminal: terminal.clone(),
            },
            Effect::StreamsEnd { terminal, lost } => EffectView::StreamsEnd {
                terminal: terminal.clone(),
                lost: *lost,
            },
            Effect::DocChanged {
                name,
                version,
                machine,
            } => EffectView::DocChanged {
                name: name.clone(),
                version: *version,
                machine: machine.clone(),
            },
            Effect::Resnapshot => EffectView::Resnapshot,
            Effect::NeedResnapshot(reason) => EffectView::NeedResnapshot {
                reason: reason.clone(),
            },
        })
        .collect()
}

/// A control message on an attach's channel. Output bytes go as raw bodies on the same
/// channel, so a `closed` message always arrives after the last byte before it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AttachMessage {
    Closed { end: AttachEndView },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AttachEndView {
    /// The core answered our detach.
    Detached,
    /// The program ended or was lost (`reason`: `exited`, `killed`, `server_stop`, ...).
    Ended {
        reason: String,
        exit_code: Option<i64>,
        signal: Option<i64>,
        /// A kept terminal that is lost gets a new shell: wait for `reattach`.
        reattach_after_restart: bool,
    },
    /// A close payload this build does not understand.
    Unparsed { raw: String },
    /// The connection ended under the stream; `code` is the `ClientError` code of the cause.
    Session { code: String },
}

impl AttachEndView {
    pub fn from_end(end: &AttachEnd) -> AttachEndView {
        match end {
            AttachEnd::Core {
                reason,
                reattach_after_restart,
            } => match reason {
                CloseReason::Detached => AttachEndView::Detached,
                CloseReason::Ended {
                    reason: text,
                    exit_code,
                    signal,
                } => AttachEndView::Ended {
                    reason: text.clone(),
                    exit_code: *exit_code,
                    signal: *signal,
                    // Worked out by the client from the terminal's `keep`; not recomputed here.
                    reattach_after_restart: *reattach_after_restart,
                },
                CloseReason::Unparsed(raw) => AttachEndView::Unparsed { raw: raw.clone() },
            },
            AttachEnd::Session(code) => AttachEndView::Session {
                code: (*code).to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yuzora_host::herdr_core::docs::ReadOnlyReason;

    #[test]
    fn herdr_core_attach_end_views_cover_every_way_a_stream_ends() {
        let json_of = |end: AttachEnd| {
            serde_json::to_value(AttachMessage::Closed {
                end: AttachEndView::from_end(&end),
            })
            .unwrap()
        };
        let core = |reason: CloseReason, reattach_after_restart: bool| AttachEnd::Core {
            reason,
            reattach_after_restart,
        };
        assert_eq!(
            json_of(core(CloseReason::Detached, false)),
            json!({"type": "closed", "end": {"kind": "detached"}})
        );
        assert_eq!(
            json_of(core(
                CloseReason::parse(br#"{"reason":"exited","exit_code":3}"#),
                false
            )),
            json!({"type": "closed", "end": {"kind": "ended", "reason": "exited",
                "exitCode": 3, "signal": null, "reattachAfterRestart": false}})
        );
        assert_eq!(
            json_of(core(
                CloseReason::parse(br#"{"reason":"holder_lost"}"#),
                true
            )),
            json!({"type": "closed", "end": {"kind": "ended", "reason": "holder_lost",
                "exitCode": null, "signal": null, "reattachAfterRestart": true}})
        );
        assert_eq!(
            json_of(core(CloseReason::parse(b"not json"), false)),
            json!({"type": "closed", "end": {"kind": "unparsed", "raw": "not json"}})
        );
        assert_eq!(
            json_of(AttachEnd::Session("connection_lost")),
            json!({"type": "closed", "end": {"kind": "session", "code": "connection_lost"}})
        );
    }

    #[test]
    fn herdr_core_attach_end_view_takes_the_clients_reattach_flag_as_it_is() {
        // The client decides from the terminal's `keep`; the view must not decide again from
        // the reason alone (`holder_lost` would then always say "comes back").
        let flag_of = |reason: &[u8], flag: bool| {
            let view = AttachEndView::from_end(&AttachEnd::Core {
                reason: CloseReason::parse(reason),
                reattach_after_restart: flag,
            });
            match view {
                AttachEndView::Ended {
                    reattach_after_restart,
                    ..
                } => reattach_after_restart,
                other => panic!("not an end: {other:?}"),
            }
        };
        for reason in [
            &br#"{"reason":"holder_lost"}"#[..],
            br#"{"reason":"exited","exit_code":0}"#,
        ] {
            assert!(flag_of(reason, true));
            assert!(!flag_of(reason, false));
        }
    }

    #[test]
    fn herdr_core_effect_views_carry_the_record_only_when_it_is_known() {
        let records = vec![json!({"address": "t1", "title": "one"})];
        let effects = [
            Effect::Added("t1".into()),
            Effect::StatusChanged("t2".into()),
            Effect::Updated("t1".into()),
            Effect::Removed("t3".into()),
            Effect::StreamsEnd {
                terminal: "t1".into(),
                lost: true,
            },
            Effect::Resnapshot,
            Effect::NeedResnapshot("gap".into()),
        ];
        assert!(effects.iter().any(needs_record));
        let views = view_effects(&effects, Some(&records));
        let json = serde_json::to_value(&views).unwrap();
        assert_eq!(
            json[0],
            json!({"kind": "added", "terminal": "t1", "record": records[0]})
        );
        assert_eq!(
            json[1],
            json!({"kind": "statusChanged", "terminal": "t2", "record": null})
        );
        assert_eq!(
            json[2],
            json!({"kind": "updated", "terminal": "t1", "record": records[0]})
        );
        assert_eq!(json[3], json!({"kind": "removed", "terminal": "t3"}));
        assert_eq!(
            json[4],
            json!({"kind": "streamsEnd", "terminal": "t1", "lost": true})
        );
        assert_eq!(json[5], json!({"kind": "resnapshot"}));
        assert_eq!(json[6], json!({"kind": "needResnapshot", "reason": "gap"}));
        // Updated alone is enough reason to read the store.
        assert!(needs_record(&Effect::Updated("t1".into())));
        // Without the store (the connection is not installed yet) there are no records.
        let bare = serde_json::to_value(view_effects(&effects[..1], None)).unwrap();
        assert_eq!(bare[0]["record"], json!(null));
    }

    #[test]
    fn herdr_core_events_are_tagged_and_carry_their_epoch() {
        let snapshot = serde_json::to_value(CoreEvent::Snapshot {
            epoch: 4,
            terminals: vec![json!({"address": "t1"})],
        })
        .unwrap();
        assert_eq!(snapshot["type"], "snapshot");
        assert_eq!(snapshot["epoch"], 4);
        let layout = serde_json::to_value(CoreEvent::layout(
            4,
            &LayoutSnapshot {
                version: 9,
                state: LayoutState::Empty,
            },
        ))
        .unwrap();
        assert_eq!(
            layout,
            json!({"type": "layout", "epoch": 4, "version": 9, "state": {"kind": "empty"}})
        );
    }

    #[test]
    fn herdr_core_read_only_layouts_reach_the_frontend_with_the_hosts_flat_shape() {
        // `kind` says "readOnly"; `reason` says why. Host 1.x tagged both as `kind`, which
        // serialized a duplicate key.
        let read_only = |reason: ReadOnlyReason| {
            serde_json::to_value(CoreEvent::layout(
                4,
                &LayoutSnapshot {
                    version: 9,
                    state: LayoutState::ReadOnly(reason),
                },
            ))
            .unwrap()["state"]
                .clone()
        };
        assert_eq!(
            read_only(ReadOnlyReason::NewerSchema { schema: 2 }),
            json!({"kind": "readOnly", "reason": "newerSchema", "schema": 2})
        );
        assert_eq!(
            read_only(ReadOnlyReason::ForeignApp {
                app: Some("other".into())
            }),
            json!({"kind": "readOnly", "reason": "foreignApp", "app": "other"})
        );
        assert_eq!(
            read_only(ReadOnlyReason::Invalid {
                message: "no schema".into()
            }),
            json!({"kind": "readOnly", "reason": "invalid", "message": "no schema"})
        );
    }
}
