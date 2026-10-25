//! Web Push triggers. The gateway never talks to the network and never holds the
//! VAPID key: it keeps the list of devices that hold a subscription, notices that
//! an agent became blocked or done, and asks the desktop to deliver the push.
use crate::{
    admin::{Event, Init, PushDevice, PushPayload, Subscription},
    devices,
    gateway::Gateway,
    herdr::{self, Resource},
    herdr_jobs::Lease,
    secrets,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
    time::Instant,
};
use yuzora_host::herdr_service::{HerdrSubscriptionEvent, OnSubscriptionEvent};

const ENDPOINT_BYTES: usize = 2048;
/// A subscription may only point at a browser vendor's push service.
const PUSH_SERVICES: [&str; 4] = [
    "push.apple.com",
    "fcm.googleapis.com",
    "updates.push.services.mozilla.com",
    "notify.windows.com",
];
const MAX_DEVICES: usize = 64;
/// Stored subscriptions per device, one per browser.
const MAX_ENDPOINTS: usize = 4;
/// Events buffered between the Host reader thread and a watcher.
const EVENTS: usize = 64;
/// How long a phone's subscribe waits for the desktop to say it stored it.
const ACK_WAIT: Duration = Duration::from_secs(5);
/// Agent and Space names go to the desktop (and into a notification) bounded.
const LABEL_CHARS: usize = 128;

pub(crate) struct Push {
    /// The VAPID public key phones subscribe with. `None`: push is unavailable.
    vapid: Option<String>,
    /// Sessions to watch: the allowlist.
    sessions: Vec<String>,
    /// Devices that hold a stored subscription, each with its endpoint ids. The
    /// desktop owns the truth and replaces it; revoking a device also removes it
    /// here.
    devices: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// True while `devices` is not empty: the watchers run exactly then.
    active: watch::Sender<bool>,
    /// Subscribes waiting for the desktop's `push.ack`, by request id.
    acks: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>,
    next_request: AtomicU64,
}

/// The reply of a `push.subscribe`, once the desktop has answered or the wait ended.
pub(crate) type Subscribed = Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send>>;

/// The id of a subscription endpoint, as the desktop computes it: unpadded
/// base64url of the SHA-256 of its UTF-8 bytes. Endpoints are bearer URLs and
/// are never kept or logged here; only this id is.
fn endpoint_id(endpoint: &str) -> String {
    secrets::encode(&secrets::hash(endpoint.as_bytes()))
}

/// The devices a list names, each with its endpoint ids. `None` for a list that
/// is longer than any device list, repeats a device, or gives one 0 or more than
/// 4 endpoints or an id that is not an endpoint id.
fn stored(list: &[PushDevice]) -> Option<BTreeMap<&str, BTreeSet<String>>> {
    let mut devices = BTreeMap::new();
    if list.len() > MAX_DEVICES {
        return None;
    }
    for device in list {
        if !(1..=MAX_ENDPOINTS).contains(&device.endpoints.len())
            || !device
                .endpoints
                .iter()
                .all(|id| devices::decode::<32>(id).is_some())
            || devices
                .insert(
                    device.device_id.as_str(),
                    device.endpoints.iter().cloned().collect(),
                )
                .is_some()
        {
            return None;
        }
    }
    Some(devices)
}

impl Push {
    pub(crate) fn new(init: &Init) -> Result<Self, &'static str> {
        let (vapid, devices) = match &init.push {
            None => (None, BTreeMap::new()),
            Some(push) => {
                let Some(stored) =
                    stored(&push.devices).filter(|_| point_ok(&push.vapid_public_key))
                else {
                    return Err("invalid-push");
                };
                let known: HashSet<_> = init.devices.iter().map(|d| d.device_id.as_str()).collect();
                (
                    Some(push.vapid_public_key.clone()),
                    stored
                        .into_iter()
                        .filter(|(id, _)| known.contains(id))
                        .map(|(id, endpoints)| (id.to_owned(), endpoints))
                        .collect(),
                )
            }
        };
        Ok(Self {
            vapid,
            sessions: init
                .session_allowlist
                .iter()
                .filter(|session| herdr::valid_id(session))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            active: watch::channel(!devices.is_empty()).0,
            devices: Mutex::new(devices),
            acks: Arc::default(),
            next_request: AtomicU64::new(1),
        })
    }
    pub(crate) fn available(&self) -> bool {
        self.vapid.is_some()
    }
    /// `hello.push`
    pub(crate) fn hello(&self) -> Value {
        self.vapid
            .as_ref()
            .map_or(Value::Null, |key| json!({"vapidPublicKey":key}))
    }
    /// Swaps the whole set; devices that are not known are ignored. False (nothing
    /// changed) when push is unavailable or the list breaks a limit (see `stored`).
    pub(crate) fn replace(&self, list: &[PushDevice], known: impl Fn(&str) -> bool) -> bool {
        let Some(stored) = stored(list).filter(|_| self.vapid.is_some()) else {
            return false;
        };
        let mut devices = self.devices.lock().unwrap();
        *devices = stored
            .into_iter()
            .filter(|(id, _)| known(id))
            .map(|(id, endpoints)| (id.to_owned(), endpoints))
            .collect();
        self.active.send_replace(!devices.is_empty());
        true
    }
    pub(crate) fn remove(&self, id: &str) {
        let mut devices = self.devices.lock().unwrap();
        if devices.remove(id).is_some() {
            self.active.send_replace(!devices.is_empty());
        }
    }
    /// The desktop's `push.ack`: whether it stored the subscription of that
    /// request. An answer nobody waits for any more is ignored.
    pub(crate) fn ack(&self, request: u64, stored: bool) {
        if let Some(waiting) = self.acks.lock().unwrap().remove(&request) {
            let _ = waiting.send(stored);
        }
    }
    /// `push.subscribe`: asks the desktop to store the subscription. The reply
    /// is a success only once the desktop says it persisted it; it is a
    /// failure when the desktop says it did not, or says nothing in time.
    pub(crate) fn subscribe(
        &self,
        gateway: &Gateway,
        device: &str,
        params: &Value,
    ) -> Result<Subscribed, &'static str> {
        if self.vapid.is_none() {
            return Err("push-unavailable");
        }
        let subscription = subscription(params)?;
        let request = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.acks.lock().unwrap().insert(request, answer);
        if !gateway.events.try_emit(Event::push_subscription(
            device.into(),
            subscription,
            request,
        )) {
            self.acks.lock().unwrap().remove(&request);
            return Err("busy");
        }
        let acks = self.acks.clone();
        Ok(Box::pin(async move {
            let answer = tokio::time::timeout(ACK_WAIT, answered).await;
            acks.lock().unwrap().remove(&request);
            match answer {
                Ok(Ok(true)) => Ok(json!({"subscribed":true})),
                Ok(Ok(false)) => Err("push-store-failed"),
                _ => Err("push-unconfirmed"),
            }
        }))
    }
    /// The other `push.*` WS methods; `Ok` is the reply's result.
    pub(crate) fn call(
        &self,
        gateway: &Gateway,
        device: &str,
        method: &str,
        params: &Value,
    ) -> Result<Value, &'static str> {
        match method {
            "push.status" => {
                let endpoint = status_endpoint(params)?;
                // Only this browser's own endpoint counts: another browser of the
                // same device may hold a subscription this one does not.
                let subscribed = endpoint.is_some_and(|endpoint| {
                    self.devices
                        .lock()
                        .unwrap()
                        .get(device)
                        .is_some_and(|ids| ids.contains(&endpoint_id(endpoint)))
                });
                Ok(json!({"available":self.vapid.is_some(),"subscribed":subscribed}))
            }
            "push.unsubscribe" => {
                if self.vapid.is_none() {
                    return Err("push-unavailable");
                }
                let event = Event::push_unsubscription(
                    device.into(),
                    endpoint_id(unsubscribe_endpoint(params)?),
                );
                // The desktop forgets it and answers with the new `push.devices`.
                if !gateway.events.try_emit(event) {
                    return Err("busy");
                }
                Ok(json!({"subscribed":false}))
            }
            _ => Err("unknown-method"),
        }
    }
}

/// A 65-byte uncompressed P-256 point (a leading 0x04), as unpadded base64url.
fn point_ok(value: &str) -> bool {
    devices::decode::<65>(value).is_some_and(|point| point[0] == 0x04)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscribeParams {
    endpoint: String,
    keys: Keys,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Keys {
    p256dh: String,
    auth: String,
}
fn subscription(params: &Value) -> Result<Subscription, &'static str> {
    let SubscribeParams { endpoint, keys } =
        SubscribeParams::deserialize(params).map_err(|_| "invalid-params")?;
    // serde would also take `keys` as a two-element array.
    if !params["keys"].is_object()
        || !endpoint_ok(&endpoint)
        || !point_ok(&keys.p256dh)
        || devices::decode::<16>(&keys.auth).is_none()
    {
        return Err("invalid-params");
    }
    Ok(Subscription {
        endpoint,
        p256dh: keys.p256dh,
        auth: keys.auth,
    })
}
/// `push.status {endpoint: string|null}`: no endpoint (also an absent one or no
/// params at all) asks about nothing.
fn status_endpoint(params: &Value) -> Result<Option<&str>, &'static str> {
    if params.is_null() {
        return Ok(None);
    }
    let object = params.as_object().ok_or("invalid-params")?;
    if object.len() > usize::from(object.contains_key("endpoint")) {
        return Err("invalid-params");
    }
    match object.get("endpoint") {
        None | Some(Value::Null) => Ok(None),
        Some(endpoint) => endpoint_param(endpoint).map(Some),
    }
}
/// `push.unsubscribe {endpoint: string}`
fn unsubscribe_endpoint(params: &Value) -> Result<&str, &'static str> {
    match params.as_object() {
        Some(object) if object.len() == 1 => {
            endpoint_param(object.get("endpoint").ok_or("invalid-params")?)
        }
        _ => Err("invalid-params"),
    }
}
/// The endpoint of a lookup is only ever hashed, so it needs no other checks.
fn endpoint_param(endpoint: &Value) -> Result<&str, &'static str> {
    endpoint
        .as_str()
        .filter(|endpoint| endpoint.len() <= ENDPOINT_BYTES)
        .ok_or("invalid-params")
}

/// `https`, visible ASCII only, no userinfo, port or fragment, and a host that is
/// a push service or a subdomain of one. The desktop sends to this URL.
fn endpoint_ok(endpoint: &str) -> bool {
    let Some(rest) = endpoint.strip_prefix("https://") else {
        return false;
    };
    if endpoint.len() > ENDPOINT_BYTES
        || !endpoint.bytes().all(|b| b.is_ascii_graphic())
        || endpoint.contains(['#', '\\'])
    {
        return false;
    }
    // `@` and `:` cannot be in a DNS name, so userinfo and ports end here.
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    devices::valid_rp_id(host)
        && PUSH_SERVICES.iter().any(|service| {
            host == *service
                || host
                    .strip_suffix(*service)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
}

fn status_name(status: &str) -> &'static str {
    match status {
        "idle" => "idle",
        "working" => "working",
        "blocked" => "blocked",
        "done" => "done",
        _ => "unknown",
    }
}

/// An agent that just entered `blocked` or `done`.
struct Notice {
    kind: &'static str,
    pane: String,
    workspace: Option<String>,
    title: String,
    space: Option<String>,
}
impl Notice {
    fn payload(self, session: &str) -> PushPayload {
        PushPayload {
            kind: self.kind,
            session: session.into(),
            pane_id: self.pane,
            workspace_id: self.workspace,
            title: self.title,
            space: self.space,
        }
    }
}

struct Pane {
    status: &'static str,
    workspace: Option<String>,
    title: String,
    space: Option<String>,
}
impl Pane {
    /// `title` is the agent's name, else its kind, else the pane's label, else the id.
    fn read(snapshot: &Value, pane: &Value, id: &str) -> Self {
        let text = |value: &Value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| text.chars().take(LABEL_CHARS).collect::<String>())
        };
        let list = |key: &str| snapshot[key].as_array().into_iter().flatten();
        let agent = list("agents").find(|agent| agent["paneId"] == id);
        let workspace = pane["workspaceId"].as_str().map(str::to_owned);
        Self {
            status: status_name(pane["status"].as_str().unwrap_or_default()),
            title: agent
                .and_then(|agent| text(&agent["name"]).or_else(|| text(&agent["agent"])))
                .or_else(|| text(&pane["label"]))
                .unwrap_or_else(|| id.into()),
            space: workspace
                .as_deref()
                .and_then(|id| list("workspaces").find(|space| space["id"] == id))
                .and_then(|space| text(&space["label"])),
            workspace,
        }
    }
    fn notice(&self, id: &str) -> Option<Notice> {
        matches!(self.status, "blocked" | "done").then(|| Notice {
            kind: self.status,
            pane: id.into(),
            workspace: self.workspace.clone(),
            title: self.title.clone(),
            space: self.space.clone(),
        })
    }
}

/// The last known agent status of every pane of one session.
#[derive(Default)]
struct Seen {
    panes: HashMap<String, Pane>,
}
impl Seen {
    /// A pane this has not seen yet is only a starting point, never news: that
    /// makes the first snapshot silent, and after a Herdr restart (every pane is
    /// new) the phone does not ring once per pane.
    fn snapshot(&mut self, snapshot: &Value) -> Vec<Notice> {
        let mut notices = Vec::new();
        let mut panes = HashMap::new();
        for pane in snapshot["panes"].as_array().into_iter().flatten() {
            let Some(id) = pane["id"].as_str().filter(|id| herdr::valid_id(id)) else {
                continue;
            };
            let now = Pane::read(snapshot, pane, id);
            if self
                .panes
                .get(id)
                .is_some_and(|before| before.status != now.status)
            {
                notices.extend(now.notice(id));
            }
            panes.insert(id.to_owned(), now);
        }
        self.panes = panes;
        notices
    }
    fn status(&mut self, id: &str, status: &'static str) -> Option<Notice> {
        let pane = self.panes.get_mut(id)?;
        let before = std::mem::replace(&mut pane.status, status);
        if before == status {
            return None;
        }
        pane.notice(id)
    }
}

enum Signal {
    Status(String, &'static str),
    Exited(String),
    /// Panes, tabs or Spaces changed (a title or cwd update is not topology).
    Topology,
    Down,
}
impl Signal {
    fn of(event: HerdrSubscriptionEvent) -> Option<Self> {
        match event {
            HerdrSubscriptionEvent::AgentStatusChanged {
                pane_id,
                agent_status,
                ..
            } => Some(Self::Status(pane_id, status_name(&agent_status))),
            HerdrSubscriptionEvent::PaneExited { pane_id, .. } => Some(Self::Exited(pane_id)),
            HerdrSubscriptionEvent::TopologyChanged { kind, .. } if kind != "pane.updated" => {
                Some(Self::Topology)
            }
            HerdrSubscriptionEvent::Disconnected { .. } | HerdrSubscriptionEvent::Error { .. } => {
                Some(Self::Down)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Pace {
    /// Pause before looking again after a topology change: a burst is one look.
    settle: Duration,
    /// First wait after a failure; it doubles up to `longest`.
    first: Duration,
    longest: Duration,
}
impl Pace {
    pub(crate) const DEFAULT: Self = Self {
        settle: Duration::from_millis(500),
        first: Duration::from_secs(5),
        longest: Duration::from_secs(60),
    };
}
struct Backoff {
    pace: Pace,
    delay: Duration,
    /// Since when the watcher has been up without a failure. Only a failure
    /// clears it: a topology change starts a new cycle, not a new streak.
    healthy_since: Option<Instant>,
}
impl Backoff {
    fn new(pace: Pace) -> Self {
        Self {
            pace,
            delay: pace.first,
            healthy_since: None,
        }
    }
    /// A cycle that began at `began` was up: its subscription took effect.
    fn up(&mut self, began: Instant) {
        self.healthy_since.get_or_insert(began);
    }
    /// How long to wait after a failure at `now`. A watcher that stayed up for the
    /// longest wait has earned a quick first retry, whatever kind of failure ends
    /// it and however many cycles the streak was cut into.
    fn after(&mut self, now: Instant) -> Duration {
        if self
            .healthy_since
            .take()
            .is_some_and(|since| now - since >= self.pace.longest)
        {
            self.delay = self.pace.first;
        }
        let wait = self.delay;
        self.delay = (wait * 2).min(self.pace.longest);
        wait
    }
}

/// Runs as long as the gateway does. While some device holds a subscription it
/// keeps one watcher per allowlisted session, and none otherwise.
pub(crate) async fn run(gateway: Arc<Gateway>, stop: watch::Receiver<u16>) {
    run_paced(gateway, stop, Pace::DEFAULT).await;
}
async fn level(active: &mut watch::Receiver<bool>, on: bool) -> bool {
    active.wait_for(|now| *now == on).await.is_ok()
}
async fn run_paced(gateway: Arc<Gateway>, mut stop: watch::Receiver<u16>, pace: Pace) {
    if !gateway.push.available() {
        return;
    }
    let mut active = gateway.push.active.subscribe();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => return,
            on = level(&mut active, true) => if !on { return },
        }
        let mut watchers = JoinSet::new();
        for session in &gateway.push.sessions {
            watchers.spawn(watch_session(gateway.clone(), session.clone(), pace));
        }
        let stopped = tokio::select! {
            biased;
            _ = stop.changed() => true,
            _ = level(&mut active, false) => false,
        };
        // Dropping a watcher's subscription hands it back to the Host.
        watchers.abort_all();
        while watchers.join_next().await.is_some() {}
        if stopped {
            return;
        }
    }
}

enum End {
    Topology,
    Failed { up: bool },
}
async fn watch_session(gateway: Arc<Gateway>, session: String, pace: Pace) {
    let mut seen = Seen::default();
    let mut backoff = Backoff::new(pace);
    loop {
        let began = Instant::now();
        match cycle(&gateway, &session, &mut seen).await {
            End::Topology => {
                backoff.up(began);
                tokio::time::sleep(pace.settle).await;
            }
            End::Failed { up } => {
                if up {
                    backoff.up(began);
                }
                tokio::time::sleep(backoff.after(Instant::now())).await;
            }
        }
    }
}

/// Snapshot, subscribe to its panes, then follow the events until the panes
/// change or the subscription ends.
async fn cycle(gateway: &Gateway, session: &str, seen: &mut Seen) -> End {
    let Ok(snapshot) = fetch(gateway, session).await else {
        return End::Failed { up: false };
    };
    notify(gateway, session, seen.snapshot(&snapshot));
    let panes = pane_ids(&snapshot);
    let (events, mut signals) = mpsc::channel(EVENTS);
    let Ok(_subscription) =
        subscribe(gateway, session, panes.iter().cloned().collect(), events).await
    else {
        return End::Failed { up: false };
    };
    // Whatever changed before the subscription took effect has no event.
    if let Ok(now) = fetch(gateway, session).await {
        notify(gateway, session, seen.snapshot(&now));
        if pane_ids(&now) != panes {
            return End::Topology;
        }
    }
    while let Some(signal) = signals.recv().await {
        match signal {
            Signal::Status(pane, status) => notify(gateway, session, seen.status(&pane, status)),
            Signal::Exited(pane) => {
                seen.panes.remove(&pane);
            }
            Signal::Topology => return End::Topology,
            Signal::Down => break,
        }
    }
    End::Failed { up: true }
}

fn pane_ids(snapshot: &Value) -> BTreeSet<String> {
    snapshot["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["id"].as_str())
        .filter(|id| herdr::valid_id(id))
        .map(str::to_owned)
        .collect()
}

/// Host calls block: they run on the gateway's bounded executor, under a key of
/// their own so peers' limits are not the watchers'.
async fn job<T: Send + 'static>(
    gateway: &Gateway,
    session: &str,
    work: impl FnOnce() -> Result<T, &'static str> + Send + 'static,
) -> Result<T, &'static str> {
    gateway
        .herdr_jobs
        .submit_device(&format!("push:{session}"), false, work)?
        .await
        .unwrap_or(Err("herdr-unavailable"))
}
async fn fetch(gateway: &Gateway, session: &str) -> Result<Value, &'static str> {
    let (reads, name) = (gateway.herdr.clone(), session.to_owned());
    job(gateway, session, move || reads.snapshot(&name)).await
}
async fn subscribe(
    gateway: &Gateway,
    session: &str,
    panes: Vec<String>,
    events: mpsc::Sender<Signal>,
) -> Result<Lease, &'static str> {
    let reserved = gateway.herdr_jobs.reserve()?;
    let (reads, jobs, session_name) = (
        gateway.herdr.clone(),
        gateway.herdr_jobs.clone(),
        session.to_owned(),
    );
    job(gateway, session, move || {
        // The Host calls the sink on its reader thread, which must never wait: a
        // full buffer is an error, the Host ends the subscription and the watcher
        // sees the channel close.
        let sink: OnSubscriptionEvent = Arc::new(move |event| match Signal::of(event) {
            Some(signal) => events
                .try_send(signal)
                .map_err(|_| String::from("push-events-full")),
            None => Ok(()),
        });
        let id = reads
            .port
            .subscribe(&session_name, panes, sink)
            .map_err(|error| reads.after_error(&session_name, error))?;
        Ok(Lease::new(
            jobs,
            reads.port.clone(),
            Resource::Events(id),
            reserved,
        ))
    })
    .await
}

/// One request for the subscribed devices that may still receive it. A full
/// admin queue drops it, counted as lost.
fn notify(gateway: &Gateway, session: &str, notices: impl IntoIterator<Item = Notice>) {
    for notice in notices {
        let devices = targets(gateway);
        if devices.is_empty() {
            continue;
        }
        if !gateway
            .events
            .try_emit(Event::push_request(devices, notice.payload(session)))
        {
            crate::audit::event_lost(gateway);
        }
    }
}
/// Revoked devices are gone from the identity table and locked ones may not
/// receive anything until unlocked; both are left out whatever the set says.
fn targets(gateway: &Gateway) -> Vec<String> {
    let devices: Vec<_> = gateway
        .push
        .devices
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let Ok(identity) = gateway.identity.lock() else {
        return Vec::new();
    };
    devices
        .into_iter()
        .filter(|id| identity.device(id).is_some_and(|device| !device.locked))
        .collect()
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
