//! Fail-closed write audit; tightening notifications never block resource release.
use crate::{
    admin::{Event, EventSender, Scope},
    clock::{Clock, Timestamp},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

static NEXT: AtomicU64 = AtomicU64::new(1);
#[derive(Default)]
pub(crate) struct Deferred {
    pending: HashMap<u64, (String, Vec<Event>)>,
    dropped: Option<Dropped>,
}
#[derive(Clone, Copy)]
struct Dropped {
    lost: u64,
    lost_tightening: u64,
    rejected_omitted: u64,
    since: Timestamp,
}
impl Deferred {
    pub(crate) fn pending_for(&self, device: &str) -> bool {
        self.pending.values().any(|(d, _)| d == device)
    }
    pub(crate) fn retain(&mut self, key: u64, device: String, events: Vec<Event>, now: Timestamp) {
        if self.pending.len() < 64 || self.pending.contains_key(&key) {
            if let Some((_, old)) = self.pending.insert(key, (device, events)) {
                self.lost_tightening(old.len() as u64, now);
            }
        } else {
            self.lost_tightening(events.len() as u64, now);
        }
    }
    fn counts(&mut self, now: Timestamp) -> &mut Dropped {
        self.dropped.get_or_insert(Dropped {
            lost: 0,
            lost_tightening: 0,
            rejected_omitted: 0,
            since: now,
        })
    }
    fn lost(&mut self, count: u64, now: Timestamp) {
        let record = self.counts(now);
        record.lost = record.lost.saturating_add(count);
    }
    fn lost_tightening(&mut self, count: u64, now: Timestamp) {
        self.lost(count, now);
        let record = self.counts(now);
        record.lost_tightening = record.lost_tightening.saturating_add(count);
    }
    fn rejected_omitted(&mut self, now: Timestamp) {
        let record = self.counts(now);
        record.rejected_omitted = record.rejected_omitted.saturating_add(1);
    }
    pub(crate) fn flush(&mut self, sender: &EventSender) {
        self.pending.retain(|_, (_, events)| {
            while let Some(event) = events.first() {
                if !sender.try_emit(event.clone()) {
                    break;
                }
                events.remove(0);
            }
            !events.is_empty()
        });
        if let Some(record) = self.dropped {
            if sender.try_emit(Event::audit_dropped(
                record.lost,
                record.lost_tightening,
                record.rejected_omitted,
                record.since,
            )) {
                self.dropped = None;
            }
        }
    }
}
/// A view-scope Git read. Ordinary class: a full queue drops it, counted as lost.
pub(crate) fn git_read(
    gateway: &crate::gateway::Gateway,
    device: &str,
    peer_id: u64,
    session: &str,
    workspace_id: &str,
    method: &str,
) {
    let now = gateway.clock.now();
    let event = Event::audit(
        device.into(),
        peer_id,
        Scope::View,
        "git.read".into(),
        session.into(),
        Some(workspace_id.into()),
        json!({"method":method}),
        now,
    );
    if !gateway.events.try_emit(event) {
        gateway.deferred_audit.lock().unwrap().lost(1, now);
    }
}

/// An ordinary admin event that found the queue full: counted, never retried.
pub(crate) fn event_lost(gateway: &crate::gateway::Gateway) {
    let now = gateway.clock.now();
    gateway.deferred_audit.lock().unwrap().lost(1, now);
}

#[derive(Default)]
pub(crate) struct Rejections {
    count: u64,
    next: Option<Timestamp>,
}
/// An open control or full stream, as announced; kept until it is torn down so
/// the desktop can ask for the truth when an announcement was lost.
pub(crate) struct LiveStream {
    device: String,
    peer_id: u64,
    session: String,
    target: Option<String>,
    scope: Scope,
}
pub(crate) type LiveStreams = Arc<Mutex<std::collections::BTreeMap<u64, LiveStream>>>;
/// Open streams in `peers.snapshot` form, oldest first.
pub(crate) fn live_list(live: &LiveStreams) -> Vec<crate::admin::StreamInfo> {
    live.lock()
        .unwrap()
        .iter()
        .map(|(key, stream)| crate::admin::StreamInfo {
            stream: key.to_string(),
            device_id: stream.device.clone(),
            peer_id: stream.peer_id,
            session: stream.session.clone(),
            scope: stream.scope,
            target: stream.target.clone(),
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct Audit {
    pub(crate) device: String,
    peer_id: u64,
    pub(crate) session: String,
    pub(crate) target: Option<String>,
    pub(crate) scope: Scope,
    pub(crate) sender: EventSender,
    pub(crate) clock: Clock,
    pub(crate) deferred: Arc<Mutex<Deferred>>,
    key: u64,
    rejections: Arc<Mutex<Rejections>>,
    live: LiveStreams,
}
impl Audit {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        gateway: &crate::gateway::Gateway,
        device: &str,
        peer_id: u64,
        scope: Scope,
        session: String,
        target: Option<String>,
        rejections: Arc<Mutex<Rejections>>,
    ) -> Self {
        Self {
            device: device.into(),
            peer_id,
            session,
            target,
            scope,
            sender: gateway.events.clone(),
            clock: gateway.clock.clone(),
            deferred: gateway.deferred_audit.clone(),
            key: NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .expect("audit stream key exhausted"),
            rejections,
            live: gateway.live_streams.clone(),
        }
    }
    pub(crate) fn event(&self, action: &str, summary: Value) -> Event {
        Event::audit(
            self.device.clone(),
            self.peer_id,
            self.scope,
            action.into(),
            self.session.clone(),
            self.target.clone(),
            summary,
            self.clock.now(),
        )
    }
    pub(crate) fn admit(&self, action: &str, summary: Value) -> Result<(), &'static str> {
        self.sender
            .try_emit(self.event(action, summary))
            .then_some(())
            .ok_or("audit-unavailable")
    }
    pub(crate) fn state(&self, open: bool, takeover: bool) -> Event {
        // A stream announces once and closes once. Sequence is local to this
        // opaque, process-unique key, NOT the reusable WS-local numeric token.
        let seq = if open { 1 } else { 2 };
        if self.scope == Scope::Full {
            Event::full_state(
                self.device.clone(),
                self.peer_id,
                self.key.to_string(),
                seq,
                self.session.clone(),
                open,
            )
        } else {
            Event::control_state(
                self.device.clone(),
                self.peer_id,
                self.key.to_string(),
                seq,
                self.session.clone(),
                self.target.clone().unwrap_or_default(),
                takeover,
                open,
            )
        }
    }
    /// Announces the stream's open and records it as live until `tightened`.
    pub(crate) fn announce(&self, reservation: crate::admin::EventReservation, takeover: bool) {
        self.live.lock().unwrap().insert(
            self.key,
            LiveStream {
                device: self.device.clone(),
                peer_id: self.peer_id,
                session: self.session.clone(),
                target: self.target.clone(),
                scope: self.scope,
            },
        );
        reservation.emit(self.state(true, takeover));
    }
    pub(crate) fn reserve_announcement(
        &self,
    ) -> Result<crate::admin::EventReservation, &'static str> {
        // Reserve before Host mutation, under the same lock as tightening/flush.
        // Mandatory announcements have four permits separate from input buckets.
        // Later deferred
        // bundles belong to different, never-reused stream keys; they must not
        // suppress an already reserved successful open announcement.
        let deferred = self.deferred.lock().unwrap();
        if deferred.pending_for(&self.device) {
            return Err("audit-unavailable");
        }
        self.sender
            .reserve_announcement()
            .ok_or("audit-unavailable")
    }
    pub(crate) fn stalled(&self, phase: &'static str) {
        let event = self.event(
            "herdr-io-stalled",
            json!({"phase":phase,"reason":"stream-stalled"}),
        );
        if !self.sender.try_emit(event) {
            self.deferred.lock().unwrap().lost(1, self.clock.now());
        }
    }
    pub(crate) fn rejected(&self) {
        // Count only. At most one content-free summary per peer per ten seconds;
        // rejected traffic cannot occupy the tightening-only deferred capacity.
        let now = self.clock.now();
        // Detailed rejected-write audits are intentionally omitted; count that
        // gap immediately, even if the peer disappears before the next summary.
        self.deferred.lock().unwrap().rejected_omitted(now);
        let mut rejected = self.rejections.lock().unwrap();
        rejected.count = rejected.count.saturating_add(1);
        if rejected.next.is_none_or(|next| now >= next) {
            rejected.next = Some(now.after(std::time::Duration::from_secs(10)));
            if self
                .sender
                .try_emit(self.event("write.rejected", json!({"count":rejected.count})))
            {
                rejected.count = 0;
            }
        }
    }
    pub(crate) fn cancelled(&self, input: bool, outcome: &'static str) {
        let event = self.event(
            if input {
                "input.cancelled"
            } else {
                "write.cancelled"
            },
            json!({"outcome":outcome}),
        );
        if !self.sender.try_emit(event) {
            self.deferred.lock().unwrap().lost(1, self.clock.now());
        }
    }
    pub(crate) fn open_failed(&self, reason: &str) {
        if !self
            .sender
            .try_emit(self.event("open.failed", json!({"reason":reason})))
        {
            self.deferred.lock().unwrap().lost(1, self.clock.now());
        }
    }
    pub(crate) fn tightened(&self, reason: &str, announced: bool) {
        let action = if reason == "elevation-expired" {
            "expiry"
        } else if reason == "session-invalid" {
            "revoke"
        } else if self.scope == Scope::Full {
            "full.close"
        } else {
            "terminal.release"
        };
        let mut events = vec![self.event(action, json!({"reason":reason}))];
        if announced {
            self.live.lock().unwrap().remove(&self.key);
            events.push(self.state(false, false));
        }
        let mut deferred = self.deferred.lock().unwrap();
        while let Some(event) = events.first() {
            if !self.sender.try_emit(event.clone()) {
                break;
            }
            events.remove(0);
        }
        if !events.is_empty() {
            deferred.retain(self.key, self.device.clone(), events, self.clock.now());
        }
    }
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
